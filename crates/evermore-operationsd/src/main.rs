mod store;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use evermore_common::operations::{self, Request, Response};
use nostr::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use store::{OperationStore, RelayPhase};
use tungstenite::Message;
use tungstenite::stream::MaybeTlsStream;

const DEFAULT_RELAY_URL: &str = "ws://127.0.0.1:4869";
static OWNER_IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("serve") => serve().map_err(|e| e.to_string()),
        Some("catalog") => print_catalog().map_err(|e| e.to_string()),
        Some("invoke") => invoke_cli().map_err(|e| e.to_string()),
        Some("request") => request_cli().map_err(|e| e.to_string()),
        Some("approve") => approval_cli().map_err(|e| e.to_string()),
        Some("grant") => grant_cli().map_err(|e| e.to_string()),
        Some("revoke-grant") => revoke_grant_cli().map_err(|e| e.to_string()),
        Some("grants") => grants_cli().map_err(|e| e.to_string()),
        Some("status") => status_cli().map_err(|e| e.to_string()),
        Some("export") => export_cli().map_err(|e| e.to_string()),
        _ => {
            eprintln!(
                "usage: evermore-operationsd <serve|catalog|invoke|request|approve|grant|revoke-grant|grants|status|export>"
            );
            eprintln!("  serve --owner-identity NAME --service-identity NAME");
            eprintln!("  serve    run the catalogue and operation control plane");
            eprintln!("  catalog  print the catalogue from the running daemon as JSON");
            eprintln!("  invoke   invoke a catalogued read: invoke NAME JSON_ARGUMENTS");
            eprintln!("  request  submit a signed approval-gated write request");
            eprintln!("  approve  record a signed owner decision for an operation_id");
            eprintln!("  grant    issue an owner-signed operation grant to an agent pubkey");
            eprintln!(
                "  revoke-grant  revoke a grant immediately and publish its signed revocation"
            );
            eprintln!("  grants   list stored agent grants");
            eprintln!("  status   show the durable state for an operation_id");
            eprintln!("  export   encrypt and export the local journal to the configured relay");
            std::process::exit(2);
        }
    };

    if let Err(error) = result {
        eprintln!("evermore-operationsd: {error}");
        std::process::exit(1);
    }
}

fn serve() -> std::io::Result<()> {
    let owner_identity = arg_value("--owner-identity").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "serve requires --owner-identity NAME",
        )
    })?;
    let _ = OWNER_IDENTITY.set(owner_identity.clone());
    let service_identity = arg_value("--service-identity").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "serve requires --service-identity NAME",
        )
    })?;
    let owner_pubkey = load_public_key(&owner_identity).map_err(std::io::Error::other)?;
    let service_pubkey = load_public_key(&service_identity).map_err(std::io::Error::other)?;
    if owner_pubkey == service_pubkey {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "owner and operation service identities must be distinct",
        ));
    }
    let owner_pubkey_hex = owner_pubkey.to_hex();
    let store = Arc::new(OperationStore::open(owner_pubkey).map_err(std::io::Error::other)?);
    let relay_url =
        std::env::var("EVERMORE_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
    reconcile_all(&store, &service_identity, &relay_url);
    {
        let store = Arc::clone(&store);
        let service_identity = service_identity.clone();
        let relay_url = relay_url.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                reconcile_all(&store, &service_identity, &relay_url);
            }
        });
    }
    let socket_path = operations::runtime_socket_path();
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    eprintln!(
        "evermore-operationsd listening on {}",
        socket_path.display()
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let store = Arc::clone(&store);
                let owner_identity = owner_identity.clone();
                let owner_pubkey_hex = owner_pubkey_hex.clone();
                let service_identity = service_identity.clone();
                let relay_url = relay_url.clone();
                std::thread::spawn(move || {
                    if let Err(error) = handle_connection(
                        stream,
                        store,
                        &owner_identity,
                        &owner_pubkey_hex,
                        &service_identity,
                        &relay_url,
                    ) {
                        eprintln!("evermore-operationsd connection error: {error}");
                    }
                });
            }
            Err(error) => eprintln!("evermore-operationsd accept error: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(
    stream: UnixStream,
    store: Arc<OperationStore>,
    owner_identity: &str,
    owner_pubkey_hex: &str,
    service_identity: &str,
    relay_url: &str,
) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Catalog) => Response::ok(operations::catalog()),
            Ok(Request::GetDefinition { name }) => match operations::find(&name) {
                Some(definition) => Response::ok(definition),
                None => Response::err(format!("unknown operation '{name}'")),
            },
            Ok(Request::Invoke { name, arguments }) => match invoke_read(&name, &arguments) {
                Ok(data) => Response::ok(data),
                Err(error) => Response::err(error),
            },
            Ok(Request::SubmitSignedRequest { event }) => {
                match decrypt_request_content(&event, owner_identity, owner_pubkey_hex)
                    .and_then(|plaintext| store.submit(event, &plaintext))
                {
                    Ok(record) => Response::ok(reconcile_for_reply(
                        &store,
                        record,
                        service_identity,
                        relay_url,
                    )),
                    Err(error) => Response::err(error),
                }
            }
            Ok(Request::SubmitApproval { event }) => match store.approve(event) {
                Ok(record) => Response::ok(reconcile_for_reply(
                    &store,
                    record,
                    service_identity,
                    relay_url,
                )),
                Err(error) => Response::err(error),
            },
            Ok(Request::OperationStatus { operation_id }) => match store.status(&operation_id) {
                Ok(record) => Response::ok(record),
                Err(error) => Response::err(error),
            },
            Ok(Request::ListPendingOperations) => match store.all_records() {
                Ok(records) => {
                    let pending: Vec<Value> = records
                        .into_iter()
                        .filter(|r| r.status == store::Status::Pending)
                        .map(|record| {
                            serde_json::json!({
                                "operation_id": record.operation_id,
                                "actor_pubkey": record.actor_pubkey,
                                "operation": record.operation,
                                "operation_version": record.operation_version,
                                "arguments": record.arguments,
                                "created_at": record.created_at,
                                "expires_at": record.expires_at,
                                "request_published": record.request_published
                            })
                        })
                        .collect();
                    Response::ok(pending)
                }
                Err(error) => Response::err(error),
            },
            Ok(Request::OperationHistory { limit }) => match store.all_records() {
                Ok(mut records) => {
                    records.sort_by_key(|record| std::cmp::Reverse(record.created_at));
                    records.truncate(limit.clamp(1, 100));
                    let history: Vec<Value> = records.into_iter().map(|record| serde_json::json!({
                        "operation_id": record.operation_id,
                        "actor_pubkey": record.actor_pubkey,
                        "operation": record.operation,
                        "operation_version": record.operation_version,
                        "arguments": record.arguments,
                        "status": record.status,
                        "result": record.result,
                        "error": record.error,
                        "created_at": record.created_at,
                        "request_published": record.request_published,
                        "approval_event_id": record.approval_event.as_ref().and_then(|event| event.get("id")),
                        "approval_published": record.approval_published,
                        "execution_event_id": record.execution_event.as_ref().and_then(|event| event.get("id")),
                        "execution_published": record.execution_published,
                        "result_event_id": record.result_event.as_ref().and_then(|event| event.get("id")),
                        "result_published": record.result_published
                    })).collect();
                    Response::ok(history)
                }
                Err(error) => Response::err(error),
            },
            Ok(Request::ExportSnapshot) => match (store.all_records(), store.grants()) {
                (Ok(operations), Ok(grants)) => Response::ok(serde_json::json!({
                    "owner_pubkey": store.owner_pubkey_hex(),
                    "operations": operations,
                    "grants": grants,
                })),
                (Err(error), _) | (_, Err(error)) => Response::err(error),
            },
            Ok(Request::SubmitGrant { event }) => match store.register_grant(event) {
                Ok(grant) => Response::ok(reconcile_grant_for_reply(&store, grant, relay_url)),
                Err(error) => Response::err(error),
            },
            Ok(Request::RevokeGrant { event }) => match store.revoke_grant(event) {
                Ok(grant) => Response::ok(reconcile_grant_for_reply(&store, grant, relay_url)),
                Err(error) => Response::err(error),
            },
            Ok(Request::ListGrants) => match store.grants() {
                Ok(grants) => Response::ok(grants),
                Err(error) => Response::err(error),
            },
            Err(error) => Response::err(format!("invalid request: {error}")),
        };
        let mut output = serde_json::to_string(&response)
            .unwrap_or_else(|_| r#"{"status":"err","message":"serialization failed"}"#.to_string());
        output.push('\n');
        writer.write_all(output.as_bytes())?;
        writer.flush()?;
    }
    Ok(())
}

fn reconcile_for_reply(
    store: &OperationStore,
    record: store::OperationRecord,
    service_identity: &str,
    relay_url: &str,
) -> store::OperationRecord {
    match reconcile_record(store, &record.operation_id, service_identity, relay_url) {
        Ok(updated) => updated,
        Err(error) => {
            eprintln!(
                "evermore-operationsd: operation {} is persisted; relay reconciliation will retry: {error}",
                record.operation_id
            );
            let _ = store.set_relay_error(&record.operation_id, Some(error));
            store.status(&record.operation_id).unwrap_or(record)
        }
    }
}

fn reconcile_all(store: &OperationStore, service_identity: &str, relay_url: &str) {
    if let Err(error) = reconcile_grants(store, relay_url) {
        eprintln!("evermore-operationsd: grant relay reconciliation deferred: {error}");
    }
    match store.all_records() {
        Ok(records) => {
            for record in records {
                if let Err(error) =
                    reconcile_record(store, &record.operation_id, service_identity, relay_url)
                {
                    let _ = store.set_relay_error(&record.operation_id, Some(error.clone()));
                    eprintln!(
                        "evermore-operationsd: operation {} reconciliation deferred: {error}",
                        record.operation_id
                    );
                }
            }
        }
        Err(error) => eprintln!("evermore-operationsd: couldn't read operation store: {error}"),
    }
}

fn reconcile_grants(store: &OperationStore, relay_url: &str) -> Result<(), String> {
    for pending in store.pending_grant_events()? {
        publish_event(relay_url, &pending.event)?;
        store.mark_grant_published(&pending.grant_id, pending.revocation)?;
    }
    Ok(())
}

fn reconcile_grant_for_reply(
    store: &OperationStore,
    grant: store::GrantRecord,
    relay_url: &str,
) -> store::GrantRecord {
    match reconcile_grants(store, relay_url) {
        Ok(()) => store
            .grants()
            .ok()
            .and_then(|grants| {
                grants
                    .into_iter()
                    .find(|item| item.grant_id == grant.grant_id)
            })
            .unwrap_or(grant),
        Err(error) => {
            eprintln!(
                "evermore-operationsd: grant {} is persisted; relay reconciliation will retry: {error}",
                grant.grant_id
            );
            grant
        }
    }
}

fn reconcile_record(
    store: &OperationStore,
    operation_id: &str,
    service_identity: &str,
    relay_url: &str,
) -> Result<store::OperationRecord, String> {
    let mut record = store.status(operation_id)?;
    if !record.request_published
        && record
            .request_event
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.trim_start().starts_with('{'))
    {
        return store.expire_legacy_plaintext_request(operation_id);
    }
    if !record.request_published {
        publish_event(relay_url, &record.request_event)?;
        record = store.mark_published(operation_id, RelayPhase::Request)?;
    }
    if let Some(approval_event) = &record.approval_event
        && !record.approval_published
    {
        publish_event(relay_url, approval_event)?;
        record = store.mark_published(operation_id, RelayPhase::Approval)?;
    }
    if record.status == store::Status::Approved {
        if record.execution_event.is_none() {
            let content = serde_json::json!({
                "operation_id": record.operation_id,
                "operation": record.operation,
                "operation_version": record.operation_version,
                "request_event_id": record.operation_id,
                "approval_event_id": record.approval_event.as_ref().and_then(|event| event.get("id")),
                "status": "execution_attempt"
            }).to_string();
            let event = sign_event(
                service_identity,
                content,
                vec![
                    vec!["t".to_string(), "evermore-operation-execution".to_string()],
                    vec!["e".to_string(), record.operation_id.clone()],
                    vec!["p".to_string(), record.actor_pubkey.clone()],
                ],
            )?;
            record = store.save_execution_event(operation_id, event)?;
        }
        if !record.execution_published {
            let event = record
                .execution_event
                .as_ref()
                .ok_or_else(|| "execution event wasn't saved".to_string())?;
            publish_event(relay_url, event)?;
            store.mark_published(operation_id, RelayPhase::Execution)?;
        }
        if let Err(error) = execute_approved(store, operation_id) {
            let latest = store.status(operation_id)?;
            if latest.status != store::Status::Running {
                return Err(error);
            }
        }
        record = store.status(operation_id)?;
    }

    if matches!(
        record.status,
        store::Status::Succeeded
            | store::Status::Failed
            | store::Status::ExecutionUnknown
            | store::Status::Denied
            | store::Status::Cancelled
            | store::Status::Expired
            | store::Status::Revoked
    ) {
        if record.result_event.is_none() {
            let content = serde_json::json!({
                "operation_id": record.operation_id.clone(),
                "status": record.status,
            })
            .to_string();
            let event = sign_event(
                service_identity,
                content,
                vec![
                    vec!["t".to_string(), "evermore-operation-result".to_string()],
                    vec!["e".to_string(), record.operation_id.clone()],
                    vec!["p".to_string(), record.actor_pubkey.clone()],
                ],
            )?;
            record = store.save_result_event(operation_id, event)?;
        }
        if !record.result_published {
            let event = record
                .result_event
                .as_ref()
                .ok_or_else(|| "result event wasn't saved".to_string())?;
            publish_event(relay_url, event)?;
            record = store.mark_published(operation_id, RelayPhase::Result)?;
        }
    }
    if record.relay_error.is_some() {
        record = store.set_relay_error(operation_id, None)?;
    }
    Ok(record)
}

fn publish_event(relay_url: &str, event: &Value) -> Result<(), String> {
    let event_id = event
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "signed event is missing id".to_string())?;
    let (mut socket, _) = tungstenite::connect(relay_url)
        .map_err(|error| format!("couldn't connect to configured relay: {error}"))?;
    if let MaybeTlsStream::Plain(tcp) = socket.get_ref() {
        tcp.set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|error| error.to_string())?;
    }
    let message = serde_json::json!(["EVENT", event]).to_string();
    socket
        .send(Message::text(message))
        .map_err(|error| format!("couldn't publish event to control relay: {error}"))?;

    loop {
        let message = socket
            .read()
            .map_err(|error| format!("waiting for relay acknowledgement failed: {error}"))?;
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if value.get(0).and_then(Value::as_str) != Some("OK")
            || value.get(1).and_then(Value::as_str) != Some(event_id)
        {
            continue;
        }
        if value.get(2).and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        let message = value
            .get(3)
            .and_then(Value::as_str)
            .unwrap_or("event rejected");
        let lower = message.to_ascii_lowercase();
        if lower.contains("duplicate") || lower.contains("already have") {
            return Ok(());
        }
        return Err(format!(
            "control relay rejected event {event_id}: {message}"
        ));
    }
}

fn execute_approved(
    store: &OperationStore,
    operation_id: &str,
) -> Result<store::OperationRecord, String> {
    let record = store.claim_execution(operation_id)?;
    match record.operation.as_str() {
        "packages.remove" => {
            let package_id = record.arguments["package_id"]
                .as_str()
                .expect("validated string");
            let output = Command::new("evermore-npack")
                .arg("remove")
                .arg(package_id)
                .output();
            match output {
                Ok(output) if output.status.success() => store.finish_execution(
                    operation_id,
                    Ok(serde_json::json!({"package_id": package_id, "removed": true})),
                ),
                Ok(output) => store.finish_execution(
                    operation_id,
                    Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
                ),
                Err(error) => store.finish_execution(
                    operation_id,
                    Err(format!("couldn't start evermore-npack: {error}")),
                ),
            }
        }
        "ai.model.install_signed_npack" => {
            let request = evermore_common::ai::Request::InstallNpackModel {
                package_id: record.arguments["package_id"]
                    .as_str()
                    .expect("validated string")
                    .to_string(),
            };
            match send_request::<_, evermore_common::ai::Event>(
                &evermore_common::ai::runtime_infer_socket_path(),
                &request,
            ) {
                Ok(evermore_common::ai::Event::ModelInstalled { package_id, model }) => store
                    .finish_execution(
                        operation_id,
                        Ok(serde_json::json!({"package_id": package_id, "model": model})),
                    ),
                Ok(evermore_common::ai::Event::Error { message, .. }) => {
                    store.finish_execution(operation_id, Err(message))
                }
                Ok(_) => store.mark_execution_unknown(
                    operation_id,
                    "inference service returned an unexpected model import response".into(),
                ),
                Err(error) => store.mark_execution_unknown(operation_id, error),
            }
        }
        "blossom.delete" => {
            let sha256 = record.arguments["sha256"]
                .as_str()
                .expect("validated string")
                .to_ascii_lowercase();
            let result = (|| -> Result<Value, String> {
                let base = blossom_base_url()?;
                let owner = OWNER_IDENTITY
                    .get()
                    .ok_or("owner identity is unavailable")?;
                let owned = blossom_list_owned(&base, owner)?;
                if !owned.iter().any(|item| {
                    item.get("sha256")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value.eq_ignore_ascii_case(&sha256))
                }) {
                    return Err("the configured identity does not own this Blossom blob".into());
                }
                let url = format!("{base}/{sha256}");
                let auth = blossom_auth(owner, "DELETE", &url, Some(&sha256))?;
                ureq::delete(&url)
                    .header("Authorization", &auth)
                    .call()
                    .map_err(|error| format!("Blossom delete failed: {error}"))?;
                Ok(serde_json::json!({"sha256": sha256, "deleted": true}))
            })();
            store.finish_execution(operation_id, result)
        }
        "packages.install_pinned" => {
            let Some(blossom_url) = std::env::var_os("EVERMORE_BLOSSOM_URL") else {
                return store.finish_execution(
                    operation_id,
                    Err("EVERMORE_BLOSSOM_URL is required for package installation".into()),
                );
            };
            let owner_identity = OWNER_IDENTITY
                .get()
                .ok_or("owner identity is unavailable")?;
            let output = Command::new("evermore-npack")
                .arg("install")
                .arg(
                    record.arguments["package_id"]
                        .as_str()
                        .expect("validated string"),
                )
                .arg("--version")
                .arg(
                    record.arguments["version"]
                        .as_str()
                        .expect("validated string"),
                )
                .arg("--sha256")
                .arg(
                    record.arguments["sha256"]
                        .as_str()
                        .expect("validated string"),
                )
                .arg("--identity")
                .arg(owner_identity)
                .arg("--blossom-url")
                .arg(blossom_url)
                .output();
            match output {
                Ok(output) if output.status.success() => store.finish_execution(operation_id, Ok(serde_json::json!({
                    "package_id": record.arguments["package_id"], "version": record.arguments["version"], "sha256": record.arguments["sha256"]
                }))),
                Ok(output) => store.finish_execution(operation_id, Err(String::from_utf8_lossy(&output.stderr).trim().to_string())),
                Err(error) => store.finish_execution(operation_id, Err(format!("couldn't start evermore-npack: {error}"))),
            }
        }
        "settings.set" => {
            let request = evermore_common::settings::Request::Set {
                domain: record.arguments["domain"]
                    .as_str()
                    .expect("validated as string")
                    .to_string(),
                key: record.arguments["key"]
                    .as_str()
                    .expect("validated as string")
                    .to_string(),
                value: record.arguments["value"].clone(),
            };
            match send_request::<_, evermore_common::settings::Response>(
                &evermore_common::settings::runtime_socket_path(),
                &request,
            ) {
                Ok(evermore_common::settings::Response::Ok { data }) => {
                    store.finish_execution(operation_id, Ok(data))
                }
                Ok(evermore_common::settings::Response::Err { message }) => {
                    store.mark_execution_unknown(operation_id, message)
                }
                Err(error) => store.mark_execution_unknown(operation_id, error),
            }
        }
        "notifications.dismiss" => {
            let request = evermore_common::notifications::Request::Dismiss {
                notification_id: record.arguments["notification_id"]
                    .as_str()
                    .expect("validated string")
                    .to_string(),
            };
            match send_request::<_, evermore_common::notifications::Response>(
                &evermore_common::notifications::runtime_socket_path(),
                &request,
            ) {
                Ok(evermore_common::notifications::Response::Ok { data }) => {
                    store.finish_execution(operation_id, Ok(data))
                }
                Ok(evermore_common::notifications::Response::Err { message }) => {
                    store.finish_execution(operation_id, Err(message))
                }
                Err(error) => store.mark_execution_unknown(operation_id, error),
            }
        }
        "notifications.clear_recent" => {
            let request = evermore_common::notifications::Request::ClearRecent;
            match send_request::<_, evermore_common::notifications::Response>(
                &evermore_common::notifications::runtime_socket_path(),
                &request,
            ) {
                Ok(evermore_common::notifications::Response::Ok { data }) => {
                    store.finish_execution(operation_id, Ok(data))
                }
                Ok(evermore_common::notifications::Response::Err { message }) => {
                    store.finish_execution(operation_id, Err(message))
                }
                Err(error) => store.mark_execution_unknown(operation_id, error),
            }
        }
        "sway.switch_workspace" => {
            let number = record.arguments["workspace_number"]
                .as_u64()
                .expect("validated as integer");
            execute_sway_command(
                store,
                operation_id,
                &["workspace".into(), "number".into(), number.to_string()],
                serde_json::json!({"workspace_number": number}),
            )
        }
        "sway.launch_app" => {
            let program = record.arguments["program"]
                .as_str()
                .expect("validated string");
            let args: Vec<&str> = record.arguments["args"]
                .as_array()
                .expect("validated array")
                .iter()
                .map(|argument| argument.as_str().expect("validated string"))
                .collect();
            let mut command = Command::new(program);
            command
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(home) = std::env::var_os("HOME") {
                command.current_dir(home);
            }
            match command.spawn() {
                Ok(child) => store.finish_execution(
                    operation_id,
                    Ok(serde_json::json!({
                        "program": program,
                        "started": true,
                        "pid": child.id()
                    })),
                ),
                Err(error) => store.finish_execution(
                    operation_id,
                    Err(format!(
                        "couldn't launch approved application '{program}': {error}"
                    )),
                ),
            }
        }
        "sway.focus_window" => {
            let con_id = record.arguments["con_id"]
                .as_u64()
                .expect("validated as integer");
            match sway_list_windows() {
                Ok(windows)
                    if windows.iter().any(|window| {
                        window.get("con_id").and_then(Value::as_u64) == Some(con_id)
                    }) =>
                {
                    execute_sway_command(
                        store,
                        operation_id,
                        &[format!("[con_id={con_id}] focus")],
                        serde_json::json!({"con_id": con_id}),
                    )
                }
                Ok(_) => store.finish_execution(
                    operation_id,
                    Err("the requested Sway window no longer exists".into()),
                ),
                Err(error) => store.finish_execution(operation_id, Err(error)),
            }
        }
        _ => store.finish_execution(
            operation_id,
            Err(format!(
                "no executor is registered for '{}'",
                record.operation
            )),
        ),
    }
}

fn execute_sway_command(
    store: &OperationStore,
    operation_id: &str,
    args: &[String],
    result: Value,
) -> Result<store::OperationRecord, String> {
    match Command::new("swaymsg").args(args).output() {
        Ok(output) if output.status.success() => {
            let response: Value = match serde_json::from_slice(&output.stdout) {
                Ok(value) => value,
                Err(error) => {
                    return store.mark_execution_unknown(
                        operation_id,
                        format!("unreadable Sway result: {error}"),
                    );
                }
            };
            let Some(items) = response.as_array() else {
                return store.mark_execution_unknown(
                    operation_id,
                    "Sway returned an unexpected response".into(),
                );
            };
            if !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.get("success").and_then(Value::as_bool) == Some(true))
            {
                store.finish_execution(operation_id, Ok(result))
            } else if let Some(error) = items
                .iter()
                .find(|item| item.get("success").and_then(Value::as_bool) == Some(false))
            {
                let message = error
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Sway rejected the operation");
                store.finish_execution(operation_id, Err(message.to_string()))
            } else {
                store.mark_execution_unknown(
                    operation_id,
                    "Sway returned an incomplete command result".into(),
                )
            }
        }
        // Once Sway IPC received a command, a transport/error response can be
        // ambiguous. Never retry this write automatically.
        Ok(output) => store.mark_execution_unknown(
            operation_id,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ),
        Err(error) => store.finish_execution(
            operation_id,
            Err(format!("could not start swaymsg: {error}")),
        ),
    }
}

fn invoke_cli() -> std::io::Result<()> {
    let mut args = std::env::args().skip(2);
    let name = args.next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing operation name")
    })?;
    let arguments: Value = args
        .next()
        .map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?
        .unwrap_or_else(|| serde_json::json!({}));
    let response: Response = send_request(
        &operations::runtime_socket_path(),
        &Request::Invoke { name, arguments },
    )
    .map_err(std::io::Error::other)?;
    match response {
        Response::Ok { data } => println!("{}", serde_json::to_string_pretty(&data).unwrap()),
        Response::Err { message } => {
            return Err(std::io::Error::other(message));
        }
    }
    Ok(())
}

fn request_cli() -> std::io::Result<()> {
    let mut args = std::env::args().skip(2);
    let name = args
        .next()
        .ok_or_else(|| invalid_input("missing operation name"))?;
    let arguments: Value = args
        .next()
        .map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|error| invalid_input(&format!("invalid JSON arguments: {error}")))?
        .unwrap_or_else(|| serde_json::json!({}));
    let arguments =
        operations::canonicalize_input(&name, &arguments).map_err(|error| invalid_input(&error))?;
    let definition = operations::find(&name).ok_or_else(|| invalid_input("unknown operation"))?;
    if definition.access != operations::Access::Write
        || definition.approval != operations::Approval::Required
    {
        return Err(invalid_input("operation is not an approval-gated write"));
    }
    let identity =
        arg_value("--identity").ok_or_else(|| invalid_input("missing --identity NAME"))?;
    let owner =
        arg_value("--owner-pubkey").ok_or_else(|| invalid_input("missing --owner-pubkey HEX"))?;
    let plaintext = serde_json::json!({ "operation": name, "arguments": arguments }).to_string();
    let content = encrypt_text(&identity, &owner, &plaintext).map_err(std::io::Error::other)?;
    let event = sign_event(
        &identity,
        content,
        vec![
            vec!["t".to_string(), "evermore-operation-request".to_string()],
            vec!["p".to_string(), owner],
        ],
    )
    .map_err(std::io::Error::other)?;
    let response: Response = send_request(
        &operations::runtime_socket_path(),
        &Request::SubmitSignedRequest { event },
    )
    .map_err(std::io::Error::other)?;
    print_response(response)
}

fn approval_cli() -> std::io::Result<()> {
    let mut args = std::env::args().skip(2);
    let operation_id = args
        .next()
        .ok_or_else(|| invalid_input("missing operation_id"))?;
    let decision = args
        .next()
        .ok_or_else(|| invalid_input("decision must be approve, deny, or cancel"))?;
    if decision != "approve" && decision != "deny" && decision != "cancel" {
        return Err(invalid_input("decision must be approve, deny, or cancel"));
    }
    let identity =
        arg_value("--identity").ok_or_else(|| invalid_input("missing --identity OWNER"))?;
    let content = serde_json::json!({
        "operation_id": operation_id,
        "decision": decision
    })
    .to_string();
    let event = sign_event(
        &identity,
        content,
        vec![
            vec!["t".to_string(), "evermore-operation-approval".to_string()],
            vec!["e".to_string(), operation_id],
        ],
    )
    .map_err(std::io::Error::other)?;
    let response: Response = send_request(
        &operations::runtime_socket_path(),
        &Request::SubmitApproval { event },
    )
    .map_err(std::io::Error::other)?;
    print_response(response)
}

fn grant_cli() -> std::io::Result<()> {
    let mut args = std::env::args().skip(2);
    let agent_pubkey = args
        .next()
        .ok_or_else(|| invalid_input("missing agent pubkey"))?;
    let operation_list = args
        .next()
        .ok_or_else(|| invalid_input("missing operation list"))?;
    let agent =
        PublicKey::parse(&agent_pubkey).map_err(|error| invalid_input(&error.to_string()))?;
    let allowed_operations: Vec<String> = operation_list
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    if allowed_operations.is_empty() {
        return Err(invalid_input(
            "operation list must contain at least one name",
        ));
    }
    let expires_in = arg_value("--expires-in")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| invalid_input(&format!("invalid --expires-in: {error}")))
        })
        .transpose()?
        .unwrap_or(30 * 24 * 60 * 60);
    if expires_in == 0 || expires_in > 365 * 24 * 60 * 60 {
        return Err(invalid_input(
            "--expires-in must be between 1 second and 365 days",
        ));
    }
    let identity =
        arg_value("--identity").ok_or_else(|| invalid_input("missing --identity OWNER"))?;
    let expires_at = unix_time().saturating_add(expires_in);
    let content = serde_json::json!({
        "agent_pubkey": agent.to_hex(),
        "allowed_operations": allowed_operations,
        "expires_at": expires_at
    })
    .to_string();
    let event = sign_event(
        &identity,
        content,
        vec![
            vec!["t".into(), "evermore-agent-grant".into()],
            vec!["p".into(), agent.to_hex()],
        ],
    )
    .map_err(std::io::Error::other)?;
    let response: Response = send_request(
        &operations::runtime_socket_path(),
        &Request::SubmitGrant { event },
    )
    .map_err(std::io::Error::other)?;
    print_response(response)
}

fn revoke_grant_cli() -> std::io::Result<()> {
    let grant_id = std::env::args()
        .nth(2)
        .ok_or_else(|| invalid_input("missing grant_id"))?;
    let identity =
        arg_value("--identity").ok_or_else(|| invalid_input("missing --identity OWNER"))?;
    let content = serde_json::json!({ "grant_id": grant_id }).to_string();
    let event = sign_event(
        &identity,
        content,
        vec![
            vec!["t".into(), "evermore-agent-grant-revoke".into()],
            vec!["e".into(), grant_id],
        ],
    )
    .map_err(std::io::Error::other)?;
    let response: Response = send_request(
        &operations::runtime_socket_path(),
        &Request::RevokeGrant { event },
    )
    .map_err(std::io::Error::other)?;
    print_response(response)
}

fn grants_cli() -> std::io::Result<()> {
    let response: Response = send_request(&operations::runtime_socket_path(), &Request::ListGrants)
        .map_err(std::io::Error::other)?;
    print_response(response)
}

fn status_cli() -> std::io::Result<()> {
    let operation_id = std::env::args()
        .nth(2)
        .ok_or_else(|| invalid_input("missing operation_id"))?;
    let response: Result<Response, String> = send_request(
        &operations::runtime_socket_path(),
        &Request::OperationStatus {
            operation_id: operation_id.clone(),
        },
    );
    match response {
        Ok(response) => print_response(response),
        Err(daemon_error) => match store::OperationStore::read_status_offline(&operation_id) {
            Ok(record) => {
                let mut data = serde_json::to_value(record).map_err(std::io::Error::other)?;
                data["status_source"] = serde_json::json!("local_journal_offline");
                println!("{}", serde_json::to_string_pretty(&data).unwrap());
                Ok(())
            }
            Err(journal_error) => Err(std::io::Error::other(format!(
                "operation daemon unavailable ({daemon_error}); offline journal lookup failed ({journal_error})"
            ))),
        },
    }
}

fn export_cli() -> std::io::Result<()> {
    use sha2::{Digest, Sha256};

    let relay = std::env::var("EVERMORE_ACTIVITY_EXPORT_RELAY").map_err(|_| {
        invalid_input(
            "activity export is disabled; set EVERMORE_ACTIVITY_EXPORT_RELAY to the user-selected relay",
        )
    })?;
    if !(relay.starts_with("ws://") || relay.starts_with("wss://")) || relay.len() > 2048 {
        return Err(invalid_input(
            "EVERMORE_ACTIVITY_EXPORT_RELAY must be a ws:// or wss:// URL",
        ));
    }
    let identity = arg_value("--identity")
        .ok_or_else(|| invalid_input("export requires --identity SERVICE_IDENTITY"))?;
    let owner_identity = arg_value("--owner-identity")
        .ok_or_else(|| invalid_input("export requires --owner-identity OWNER_IDENTITY"))?;
    let owner_pubkey = match send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::GetPublicKey {
            identity: owner_identity,
        },
    )
    .map_err(std::io::Error::other)?
    {
        evermore_common::Response::Ok { data } => data
            .get("pubkey_hex")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| invalid_input("signer response is missing owner pubkey"))?,
        evermore_common::Response::Err { message } => return Err(std::io::Error::other(message)),
    };
    let service_pubkey = match send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::GetPublicKey {
            identity: identity.clone(),
        },
    )
    .map_err(std::io::Error::other)?
    {
        evermore_common::Response::Ok { data } => data
            .get("pubkey_hex")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| invalid_input("signer response is missing service pubkey"))?,
        evermore_common::Response::Err { message } => return Err(std::io::Error::other(message)),
    };
    if service_pubkey == owner_pubkey {
        return Err(invalid_input(
            "activity export must use a service identity distinct from the owner",
        ));
    }
    let snapshot: Response =
        send_request(&operations::runtime_socket_path(), &Request::ExportSnapshot)
            .map_err(std::io::Error::other)?;
    let snapshot = match snapshot {
        Response::Ok { data } => data,
        Response::Err { message } => return Err(std::io::Error::other(message)),
    };
    if snapshot.get("owner_pubkey").and_then(Value::as_str) != Some(&owner_pubkey) {
        return Err(invalid_input(
            "the selected owner identity doesn't match the operations daemon's configured owner",
        ));
    }
    let operation_count = snapshot
        .get("operations")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let plaintext = serde_json::json!({
        "format": "evermore-operation-journal-v1",
        "exported_at": unix_time(),
        "owner_pubkey": owner_pubkey,
        "journal": snapshot,
    })
    .to_string();
    let export_id = hex::encode(Sha256::digest(plaintext.as_bytes()));
    let chunks = utf8_chunks(&plaintext, 24 * 1024);
    let total = chunks.len();
    let mut event_ids = Vec::with_capacity(total);
    for (index, chunk) in chunks.into_iter().enumerate() {
        let ciphertext =
            encrypt_text(&identity, &owner_pubkey, chunk).map_err(std::io::Error::other)?;
        let event = sign_event(
            &identity,
            ciphertext,
            vec![
                vec!["t".into(), "evermore-operation-journal-export".into()],
                vec!["p".into(), owner_pubkey.clone()],
                vec!["export".into(), export_id.clone()],
                vec!["part".into(), (index + 1).to_string(), total.to_string()],
            ],
        )
        .map_err(std::io::Error::other)?;
        publish_event(&relay, &event).map_err(|error| {
            std::io::Error::other(format!(
                "export {export_id} stopped after {}/{} acknowledged parts: {error}",
                event_ids.len(),
                total
            ))
        })?;
        event_ids.push(
            event
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    println!(
        "{}",
        serde_json::json!({
            "exported": true,
            "encrypted": true,
            "relay_configured": true,
            "export_id": export_id,
            "event_ids": event_ids,
            "parts": total,
            "operation_count": operation_count,
        })
    );
    Ok(())
}

fn utf8_chunks(text: &str, max_bytes: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&text[start..end]);
        start = end;
    }
    if chunks.is_empty() {
        chunks.push("");
    }
    chunks
}

fn sign_event(identity: &str, content: String, tags: Vec<Vec<String>>) -> Result<Value, String> {
    match send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::SignEvent {
            identity: identity.to_string(),
            kind: 1,
            content,
            tags,
            created_at: None,
        },
    )? {
        evermore_common::Response::Ok { data } => Ok(data),
        evermore_common::Response::Err { message } => Err(message),
    }
}

fn encrypt_text(identity: &str, recipient_pubkey: &str, plaintext: &str) -> Result<String, String> {
    match send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::Nip44Encrypt {
            identity: identity.to_string(),
            recipient_pubkey: recipient_pubkey.to_string(),
            plaintext: plaintext.to_string(),
        },
    )? {
        evermore_common::Response::Ok { data } => data
            .get("ciphertext")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "signer response is missing ciphertext".to_string()),
        evermore_common::Response::Err { message } => Err(message),
    }
}

fn decrypt_request_content(
    event_value: &Value,
    owner_identity: &str,
    owner_pubkey_hex: &str,
) -> Result<String, String> {
    let event: Event = serde_json::from_value(event_value.clone())
        .map_err(|error| format!("invalid signed request event: {error}"))?;
    event
        .verify()
        .map_err(|error| format!("invalid request signature: {error}"))?;
    if event.kind != Kind::TextNote {
        return Err("operation requests must use a signed kind:1 event".into());
    }
    let has_tag = |name: &str, value: &str| {
        event.tags.iter().any(|tag| {
            tag.as_slice().first().map(String::as_str) == Some(name)
                && tag.as_slice().get(1).map(String::as_str) == Some(value)
        })
    };
    if !has_tag("t", "evermore-operation-request") || !has_tag("p", owner_pubkey_hex) {
        return Err("signed request has an invalid marker or owner tag".into());
    }
    match send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::Nip44Decrypt {
            identity: owner_identity.to_string(),
            sender_pubkey: event.pubkey.to_hex(),
            ciphertext: event.content,
        },
    )? {
        evermore_common::Response::Ok { data } => data
            .get("plaintext")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| "signer response is missing plaintext".to_string()),
        evermore_common::Response::Err { message } => Err(message),
    }
}

fn load_public_key(identity: &str) -> Result<PublicKey, String> {
    match send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::GetPublicKey {
            identity: identity.to_string(),
        },
    )? {
        evermore_common::Response::Ok { data } => {
            let pubkey = data
                .get("pubkey_hex")
                .and_then(Value::as_str)
                .ok_or_else(|| "signer response is missing pubkey_hex".to_string())?;
            PublicKey::parse(pubkey).map_err(|error| error.to_string())
        }
        evermore_common::Response::Err { message } => Err(message),
    }
}

fn blossom_base_url() -> Result<String, String> {
    let raw = std::env::var("EVERMORE_BLOSSOM_URL")
        .map_err(|_| "EVERMORE_BLOSSOM_URL is required".to_string())?;
    let base = raw.trim_end_matches('/');
    if base.len() > 2048
        || !(base.starts_with("http://") || base.starts_with("https://"))
        || base.bytes().any(|b| b.is_ascii_control())
        || base.contains('@')
    {
        return Err(
            "EVERMORE_BLOSSOM_URL must be a valid HTTP(S) origin without credentials".into(),
        );
    }
    Ok(base.to_string())
}

fn blossom_auth(
    identity: &str,
    method: &str,
    url: &str,
    payload_sha256: Option<&str>,
) -> Result<String, String> {
    let mut tags = vec![
        vec!["u".to_string(), url.to_string()],
        vec!["method".to_string(), method.to_string()],
    ];
    if let Some(sha) = payload_sha256 {
        tags.push(vec!["payload".to_string(), sha.to_string()]);
    }
    let event = match send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::SignEvent {
            identity: identity.to_string(),
            kind: 27235,
            content: String::new(),
            tags,
            created_at: None,
        },
    )? {
        evermore_common::Response::Ok { data } => data,
        evermore_common::Response::Err { message } => return Err(message),
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_vec(&event).map_err(|error| format!("encoding NIP-98 event: {error}"))?,
    );
    Ok(format!("Nostr {encoded}"))
}

fn blossom_list_owned(base: &str, identity: &str) -> Result<Vec<Value>, String> {
    let pubkey = load_public_key(identity)?.to_hex();
    let url = format!("{base}/list/{pubkey}");
    let auth = blossom_auth(identity, "GET", &url, None)?;
    let mut response = ureq::get(&url)
        .header("Authorization", &auth)
        .call()
        .map_err(|error| format!("Blossom listing failed: {error}"))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()
        .map_err(|error| format!("reading Blossom listing: {error}"))?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|error| format!("invalid Blossom listing JSON: {error}"))?;
    value
        .as_array()
        .cloned()
        .ok_or_else(|| "Blossom listing response must be an array".into())
}

fn arg_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn invalid_input(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.to_string())
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn print_response(response: Response) -> std::io::Result<()> {
    match response {
        Response::Ok { data } => println!("{}", serde_json::to_string_pretty(&data).unwrap()),
        Response::Err { message } => return Err(std::io::Error::other(message)),
    }
    Ok(())
}

fn invoke_read(name: &str, arguments: &Value) -> Result<Value, String> {
    let definition = operations::find(name).ok_or_else(|| format!("unknown operation '{name}'"))?;
    let normalized_arguments = operations::canonicalize_input(name, arguments)?;
    let arguments = &normalized_arguments;
    if definition.access != operations::Access::Read {
        return Err(format!(
            "'{name}' is a write operation and cannot run before approval support is enabled"
        ));
    }

    let data = match name {
        "identity.get_public_key" => {
            let request = evermore_common::Request::GetPublicKey {
                identity: arguments["identity"]
                    .as_str()
                    .expect("validated as string")
                    .to_string(),
            };
            match send_request::<_, evermore_common::Response>(
                &evermore_common::runtime_socket_path(),
                &request,
            )? {
                evermore_common::Response::Ok { data } => data,
                evermore_common::Response::Err { message } => return Err(message),
            }
        }
        "notifications.list_recent" => {
            match send_request::<_, evermore_common::notifications::Response>(
                &evermore_common::notifications::runtime_socket_path(),
                &evermore_common::notifications::Request::ListRecent,
            )? {
                evermore_common::notifications::Response::Ok { data } => {
                    let mut records = data.as_array().cloned().unwrap_or_default();
                    records.retain(|record| {
                        record.get("source").and_then(Value::as_str) != Some("dm")
                    });
                    Value::Array(records)
                }
                evermore_common::notifications::Response::Err { message } => return Err(message),
            }
        }
        "packages.list_installed" => {
            let output = Command::new("evermore-npack")
                .args(["installed", "--json"])
                .output()
                .map_err(|error| format!("couldn't start evermore-npack: {error}"))?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
            }
            serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("invalid evermore-npack JSON response: {error}"))?
        }
        "packages.list_available" => {
            let owner_identity = OWNER_IDENTITY
                .get()
                .ok_or("owner identity is unavailable")?;
            let relay = std::env::var("EVERMORE_RELAY_URL")
                .unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string());
            let output = Command::new("evermore-npack")
                .args([
                    "available",
                    "--identity",
                    owner_identity,
                    "--relay",
                    &relay,
                    "--json",
                ])
                .output()
                .map_err(|error| format!("couldn't start evermore-npack: {error}"))?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
            }
            serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("invalid evermore-npack JSON response: {error}"))?
        }
        "packages.explain_dependencies" => {
            let output = Command::new("evermore-npack")
                .args(["installed", "--json"])
                .output()
                .map_err(|error| format!("couldn't start evermore-npack: {error}"))?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
            }
            let manifests: Vec<evermore_npack::Manifest> =
                serde_json::from_slice(&output.stdout)
                    .map_err(|error| format!("invalid evermore-npack JSON response: {error}"))?;
            let package_id = arguments["package_id"].as_str().expect("validated string");
            let package = manifests
                .iter()
                .find(|manifest| manifest.id == package_id)
                .ok_or_else(|| format!("package '{package_id}' is not installed"))?;
            let dependencies: Vec<Value> = package
                .dependencies
                .iter()
                .map(|dependency_id| {
                    let dependency = manifests
                        .iter()
                        .find(|manifest| manifest.id == *dependency_id);
                    serde_json::json!({
                        "package_id": dependency_id,
                        "installed": dependency.is_some(),
                        "name": dependency.map(|manifest| manifest.name.as_str()),
                        "version": dependency.map(|manifest| manifest.version.as_str())
                    })
                })
                .collect();
            serde_json::json!({
                "package_id": package.id,
                "name": package.name,
                "version": package.version,
                "dependencies": dependencies
            })
        }
        "blossom.list_owned" => {
            let owner = OWNER_IDENTITY
                .get()
                .ok_or("owner identity is unavailable")?;
            Value::Array(blossom_list_owned(&blossom_base_url()?, owner)?)
        }
        "settings.get" => {
            let request = evermore_common::settings::Request::Get {
                domain: arguments["domain"]
                    .as_str()
                    .expect("validated as string")
                    .to_string(),
                key: arguments["key"]
                    .as_str()
                    .expect("validated as string")
                    .to_string(),
            };
            match send_request::<_, evermore_common::settings::Response>(
                &evermore_common::settings::runtime_socket_path(),
                &request,
            )? {
                evermore_common::settings::Response::Ok { data } => data,
                evermore_common::settings::Response::Err { message } => return Err(message),
            }
        }
        "settings.get_domain" => {
            let request = evermore_common::settings::Request::GetDomain {
                domain: arguments["domain"]
                    .as_str()
                    .expect("validated as string")
                    .to_string(),
            };
            match send_request::<_, evermore_common::settings::Response>(
                &evermore_common::settings::runtime_socket_path(),
                &request,
            )? {
                evermore_common::settings::Response::Ok { data } => data,
                evermore_common::settings::Response::Err { message } => return Err(message),
            }
        }
        "settings.list_domains" => {
            match send_request::<_, evermore_common::settings::Response>(
                &evermore_common::settings::runtime_socket_path(),
                &evermore_common::settings::Request::ListDomains,
            )? {
                evermore_common::settings::Response::Ok { data } => data,
                evermore_common::settings::Response::Err { message } => return Err(message),
            }
        }
        "ai.context.list_sources" => {
            match send_request::<_, evermore_common::ai_context::Event>(
                &evermore_common::ai_context::runtime_socket_path(),
                &evermore_common::ai_context::Request::ListSources,
            )? {
                evermore_common::ai_context::Event::Sources { sources } => {
                    serde_json::json!(sources)
                }
                evermore_common::ai_context::Event::Error { message } => return Err(message),
                _ => return Err("unexpected context service response".into()),
            }
        }
        "ai.context.get" => {
            let source = arguments["source"]
                .as_str()
                .expect("validated as string")
                .to_string();
            match send_request::<_, evermore_common::ai_context::Event>(
                &evermore_common::ai_context::runtime_socket_path(),
                &evermore_common::ai_context::Request::Get { source },
            )? {
                evermore_common::ai_context::Event::Context { data, .. } => data,
                evermore_common::ai_context::Event::Error { message } => return Err(message),
                _ => return Err("unexpected context service response".into()),
            }
        }
        "ai.index.search" => {
            let request = evermore_common::ai_index::Request::Search {
                query: arguments["query"]
                    .as_str()
                    .expect("validated as string")
                    .to_string(),
                limit: arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|limit| limit as usize),
            };
            match send_request::<_, evermore_common::ai_index::Event>(
                &evermore_common::ai_index::runtime_socket_path(),
                &request,
            )? {
                evermore_common::ai_index::Event::Results { results, .. } => {
                    serde_json::json!(results)
                }
                evermore_common::ai_index::Event::Error { message } => return Err(message),
                _ => return Err("unexpected index service response".into()),
            }
        }
        "sway.list_windows" => serde_json::json!(sway_list_windows()?),
        _ => return Err(format!("'{name}' has no operation handler")),
    };

    Ok(operations::redact_result(&definition, data))
}

fn sway_list_windows() -> Result<Vec<Value>, String> {
    let output = Command::new("swaymsg")
        .args(["-t", "get_tree", "-r"])
        .output()
        .map_err(|e| format!("running swaymsg: {e}"))?;
    if !output.status.success() {
        return Err(format!("swaymsg exited with {}", output.status));
    }
    let tree: Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("invalid Sway tree: {e}"))?;
    let mut windows = Vec::new();
    collect_sway_windows(&tree, None, &mut windows);
    Ok(windows)
}

fn collect_sway_windows(node: &Value, parent_workspace: Option<String>, output: &mut Vec<Value>) {
    let workspace = if node.get("type").and_then(Value::as_str) == Some("workspace") {
        node.get("name").and_then(Value::as_str).map(str::to_owned)
    } else {
        parent_workspace
    };
    let node_type = node.get("type").and_then(Value::as_str);
    let is_window = node_type.is_some_and(|kind| kind == "con" || kind == "floating_con")
        && (node.get("window").is_some_and(|window| !window.is_null())
            || node.get("app_id").and_then(Value::as_str).is_some());
    if is_window {
        if let Some(con_id) = node.get("id").and_then(Value::as_u64) {
            output.push(serde_json::json!({
                "con_id": con_id,
                "title": node.get("name").and_then(Value::as_str).unwrap_or(""),
                "app_id": node.get("app_id").and_then(Value::as_str).unwrap_or(""),
                "window_class": node.get("window_properties").and_then(|p| p.get("class")).and_then(Value::as_str).unwrap_or(""),
                "workspace": workspace.as_deref().unwrap_or(""),
                "focused": node.get("focused").and_then(Value::as_bool).unwrap_or(false)
            }));
        }
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                collect_sway_windows(child, workspace.clone(), output);
            }
        }
    }
}

fn send_request<T: Serialize, R: DeserializeOwned>(
    path: &std::path::Path,
    request: &T,
) -> Result<R, String> {
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("couldn't connect to {}: {error}", path.display()))?;
    let line = serde_json::to_string(request).map_err(|error| error.to_string())?;
    writeln!(stream, "{line}").map_err(|error| error.to_string())?;
    let mut response_line = String::new();
    BufReader::new(stream)
        .read_line(&mut response_line)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&response_line).map_err(|error| error.to_string())
}

fn print_catalog() -> std::io::Result<()> {
    let socket_path = operations::runtime_socket_path();
    let mut stream = UnixStream::connect(&socket_path)?;
    let request = serde_json::to_string(&Request::Catalog)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    writeln!(stream, "{request}")?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::from_str::<serde_json::Value>(&response)
                .map_err(|error| std::io::Error::other(error.to_string()))?
        )
        .map_err(|error| std::io::Error::other(error.to_string()))?
    );
    Ok(())
}
