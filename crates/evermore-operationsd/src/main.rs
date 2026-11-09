mod calendar;
mod npackd_client;
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

fn configured_relay_url() -> String {
    std::env::var("EVERMORE_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string())
}

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
    let relay_url = configured_relay_url();
    reconcile_all(&store, &service_identity, &relay_url);
    let calendar_owner = owner_pubkey;
    let calendar_identity = owner_identity.clone();
    std::thread::spawn(move || {
        loop {
            if let Err(error) = calendar::sync(&calendar_owner, &calendar_identity) {
                eprintln!("evermore-operationsd: calendar relay sync failed: {error}");
            }
            std::thread::sleep(Duration::from_secs(300));
        }
    });
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
            Ok(Request::ConfiguredOwner) => Response::ok(store.owner_pubkey_hex()),
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
                    let owner_pubkey = store.owner_pubkey_hex();
                    let pending: Vec<Value> = records
                        .into_iter()
                        .filter(|r| r.status == store::Status::Pending)
                        .map(|record| {
                            serde_json::json!({
                                "owner_pubkey": owner_pubkey,
                                "operation_id": record.operation_id,
                                "actor_pubkey": record.actor_pubkey,
                                "operation": record.operation,
                                "operation_version": record.operation_version,
                                "task_id": record.task_id,
                                "step_index": record.step_index,
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
                        "task_id": record.task_id,
                        "step_index": record.step_index,
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
    publish_event_to_relay(relay_url, event, "control relay")
}

fn publish_event_to_relay(relay_url: &str, event: &Value, relay_label: &str) -> Result<(), String> {
    let event_id = event
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "signed event is missing id".to_string())?;
    let (mut socket, _) = tungstenite::connect(relay_url)
        .map_err(|error| format!("couldn't connect to {relay_label}: {error}"))?;
    if let MaybeTlsStream::Plain(tcp) = socket.get_ref() {
        tcp.set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|error| error.to_string())?;
    }
    let message = serde_json::json!(["EVENT", event]).to_string();
    socket
        .send(Message::text(message))
        .map_err(|error| format!("couldn't publish event to {relay_label}: {error}"))?;

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
            "{relay_label} rejected event {event_id}: {message}"
        ));
    }
}

fn execute_approved(
    store: &OperationStore,
    operation_id: &str,
) -> Result<store::OperationRecord, String> {
    let record = store.claim_execution(operation_id)?;
    match record.operation.as_str() {
        "calendar.create_event" => {
            if let Err(error) = validate_calendar_event_arguments(&record.arguments) {
                return store.finish_execution(operation_id, Err(error));
            }
            match create_calendar_event(&record.arguments, operation_id) {
                Ok(result) => store.finish_execution(operation_id, Ok(result)),
                Err(error) => store.mark_execution_unknown(operation_id, error),
            }
        }
        "calendar.update_event" => {
            if let Err(error) = validate_calendar_mutation_arguments(&record.arguments) {
                return store.finish_execution(operation_id, Err(error));
            }
            match update_calendar_event(&record.arguments, operation_id) {
                Ok(result) => store.finish_execution(operation_id, Ok(result)),
                Err(error) => store.mark_execution_unknown(operation_id, error),
            }
        }
        "calendar.delete_event" => {
            if let Err(error) = validate_calendar_delete_arguments(&record.arguments) {
                return store.finish_execution(operation_id, Err(error));
            }
            match delete_calendar_event(&record.arguments, operation_id) {
                Ok(result) => store.finish_execution(operation_id, Ok(result)),
                Err(error) => store.mark_execution_unknown(operation_id, error),
            }
        }
        "packages.remove" => {
            let publisher = record.arguments["publisher"]
                .as_str()
                .expect("validated string");
            let name = record.arguments["name"].as_str().expect("validated string");
            let result = npackd_client::call(
                "Remove",
                serde_json::json!({"package": format!("{publisher}/{name}"), "user": true}),
            )
            .map(|_| serde_json::json!({"publisher": publisher, "name": name, "removed": true}));
            store.finish_execution(operation_id, result)
        }
        "ai.model.install_signed_npack" => {
            let publisher = record.arguments["publisher"]
                .as_str()
                .expect("validated string")
                .to_string();
            let name = record.arguments["name"]
                .as_str()
                .expect("validated string")
                .to_string();
            let request = evermore_common::ai::Request::InstallNpackModel {
                publisher: publisher.clone(),
                name: name.clone(),
            };
            match send_request::<_, evermore_common::ai::Event>(
                &evermore_common::ai::runtime_infer_socket_path(),
                &request,
            ) {
                Ok(evermore_common::ai::Event::ModelInstalled { model, .. }) => store
                    .finish_execution(
                        operation_id,
                        Ok(serde_json::json!({"publisher": publisher, "name": name, "model": model})),
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
            let publisher = record.arguments["publisher"]
                .as_str()
                .expect("validated string")
                .to_string();
            let name = record.arguments["name"]
                .as_str()
                .expect("validated string")
                .to_string();
            let version = record.arguments["version"]
                .as_str()
                .expect("validated string")
                .to_string();
            let sha256 = record.arguments["sha256"]
                .as_str()
                .expect("validated string")
                .to_string();
            let reference = format!("{publisher}/{name}");
            // npack's own `requirement` only pins a version, not a hash
            // (it has no sha256-pin parameter) -- verify the installed
            // record's sha256 ourselves and roll back on a mismatch, so
            // this still only takes effect when the exact bytes approved
            // match what npack actually verified and installed.
            let result: Result<serde_json::Value, String> = npackd_client::call_typed::<
                npackd_client::InstalledPackage,
            >(
                "Install",
                serde_json::json!({
                    "package": reference,
                    "requirement": format!("={version}"),
                    "user": true,
                }),
            )
            .and_then(|installed| {
                if installed.sha256.eq_ignore_ascii_case(&sha256) {
                    Ok(serde_json::json!({
                        "publisher": publisher, "name": name, "version": version, "sha256": sha256
                    }))
                } else {
                    let rollback_error = format!(
                        "installed {reference} v{version} hash {} did not match the approved {sha256} -- removed it",
                        installed.sha256
                    );
                    match npackd_client::call(
                        "Remove",
                        serde_json::json!({"package": reference, "user": true}),
                    ) {
                        Ok(_) => Err(rollback_error),
                        Err(remove_error) => Err(format!(
                            "{rollback_error}; removing it afterwards also failed: {remove_error}"
                        )),
                    }
                }
            });
            store.finish_execution(operation_id, result)
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

fn create_calendar_event(arguments: &Value, operation_id: &str) -> Result<Value, String> {
    let title = arguments["title"]
        .as_str()
        .ok_or("calendar event title is missing")?;
    let start = arguments["start"]
        .as_u64()
        .ok_or("calendar event start must be Unix seconds")?;
    let end = arguments["end"]
        .as_u64()
        .ok_or("calendar event end must be Unix seconds")?;
    if end <= start || end.saturating_sub(start) > 366 * 86400 {
        return Err("calendar event end must follow start and span no more than 366 days".into());
    }
    let visibility = arguments["visibility"]
        .as_str()
        .ok_or("calendar visibility is missing")?;
    if !matches!(visibility, "public" | "private") {
        return Err("calendar visibility must be public or private".into());
    }
    let relay_url = arguments["relay_url"]
        .as_str()
        .ok_or("calendar relay destination is missing")?;
    if !(relay_url.starts_with("wss://")
        || relay_url.starts_with("ws://127.0.0.1:")
        || relay_url.starts_with("ws://localhost:")
        || relay_url.starts_with("ws://[::1]:"))
    {
        return Err("calendar relays must use wss://; unencrypted ws:// is allowed only for loopback testing".into());
    }
    if !std::env::var("EVERMORE_CALENDAR_RELAYS")
        .unwrap_or_default()
        .split([',', ';'])
        .map(str::trim)
        .any(|configured| configured == relay_url)
    {
        return Err("relay destination is not in EVERMORE_CALENDAR_RELAYS".into());
    }
    let timezone = arguments["timezone"]
        .as_str()
        .ok_or("calendar event time zone is missing")?;
    if !valid_time_zone(timezone) {
        return Err("calendar time zone must name an installed IANA time zone".into());
    }
    let mut tags = vec![
        vec!["d".into(), operation_id.into()],
        vec!["title".into(), title.into()],
        vec!["start".into(), start.to_string()],
        vec!["end".into(), end.to_string()],
        vec!["start_tzid".into(), timezone.into()],
        vec!["end_tzid".into(), timezone.into()],
    ];
    for day in start / 86400..=((end - 1) / 86400) {
        tags.push(vec!["D".into(), day.to_string()]);
    }
    if let Some(summary) = arguments.get("description").and_then(Value::as_str)
        && !summary.is_empty()
    {
        tags.push(vec!["summary".into(), summary.into()]);
    }
    if let Some(coordinate) = arguments.get("calendar").and_then(Value::as_str)
        && !coordinate.is_empty()
    {
        let parts: Vec<_> = coordinate.splitn(3, ':').collect();
        if parts.len() != 3
            || parts[0] != "31924"
            || parts[2].is_empty()
            || coordinate.contains(['\n', '\r', '\0'])
        {
            return Err("calendar must be a kind-31924 address coordinate".into());
        }
        let calendar_pubkey = PublicKey::parse(parts[1])
            .map_err(|error| format!("invalid calendar owner pubkey: {error}"))?;
        tags.push(vec![
            "a".into(),
            format!("31924:{}:{}", calendar_pubkey.to_hex(), parts[2]),
        ]);
    }
    if let Some(participants) = arguments.get("participants").and_then(Value::as_array) {
        for participant in participants {
            let pubkey = participant
                .as_str()
                .ok_or("participant pubkey must be a string")?;
            let pubkey = PublicKey::parse(pubkey)
                .map_err(|error| format!("invalid participant pubkey: {error}"))?;
            tags.push(vec!["p".into(), pubkey.to_hex()]);
        }
    }
    if visibility == "private" {
        let participants = arguments
            .get("participants")
            .and_then(Value::as_array)
            .ok_or("private calendar events require at least one recipient pubkey")?;
        if participants.is_empty() {
            return Err("private calendar events require at least one recipient pubkey".into());
        }
        return create_private_calendar_event(
            arguments,
            operation_id,
            &tags,
            relay_url,
            title,
            start,
            end,
            timezone,
        );
    }
    let owner = OWNER_IDENTITY
        .get()
        .ok_or("owner identity is unavailable")?;
    let signed: evermore_common::Response = send_request(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::SignEvent {
            identity: owner.clone(),
            kind: 31923,
            content: arguments
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            tags,
            created_at: None,
        },
    )?;
    let event = match signed {
        evermore_common::Response::Ok { data } => data,
        evermore_common::Response::Err { message } => return Err(message),
    };
    let event_id = event
        .get("id")
        .and_then(Value::as_str)
        .ok_or("signer response is missing event id")?
        .to_string();
    publish_event_to_relay(relay_url, &event, "calendar relay")?;
    publish_event_to_relay(&configured_relay_url(), &event, "local control-plane relay")?;
    Ok(serde_json::json!({
        "event_id": event_id,
        "coordinate": format!("31923:{}:{operation_id}", event.get("pubkey").and_then(Value::as_str).unwrap_or("")),
        "kind": 31923,
        "visibility": visibility,
        "relay_url": relay_url,
        "published": true
    }))
}

fn create_private_calendar_event(
    arguments: &Value,
    operation_id: &str,
    tags: &[Vec<String>],
    relay_url: &str,
    title: &str,
    start: u64,
    end: u64,
    timezone: &str,
) -> Result<Value, String> {
    let owner = OWNER_IDENTITY
        .get()
        .ok_or("owner identity is unavailable")?;
    let owner_pubkey = match send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::GetPublicKey {
            identity: owner.clone(),
        },
    )? {
        evermore_common::Response::Ok { data } => data
            .get("pubkey_hex")
            .and_then(Value::as_str)
            .ok_or("signer response is missing owner pubkey")?
            .to_string(),
        evermore_common::Response::Err { message } => return Err(message),
    };
    let owner_key = PublicKey::parse(&owner_pubkey).map_err(|error| error.to_string())?;
    let mut builder = EventBuilder::new(
        Kind::from(31923),
        arguments
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    for raw in tags {
        builder = builder.tag(Tag::parse(raw.clone()).map_err(|error| error.to_string())?);
    }
    let mut rumor = builder.finalize_unsigned(owner_key);
    rumor.ensure_id();
    let rumor_id = rumor
        .id
        .as_ref()
        .ok_or("NIP-59 rumor is missing its event id")?
        .to_hex();
    let recipients = arguments
        .get("participants")
        .and_then(Value::as_array)
        .ok_or("private calendar events require at least one recipient pubkey")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "participant pubkey must be a string".to_string())
                .and_then(|value| PublicKey::parse(value).map_err(|error| error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let wrap_ids = publish_private_rumor(owner, owner_key, rumor, recipients, relay_url)?;
    Ok(serde_json::json!({
        "event_id": rumor_id,
        "coordinate": format!("31923:{owner_pubkey}:{operation_id}"),
        "kind": 31923,
        "visibility": "private",
        "relay_url": relay_url,
        "recipient_count": wrap_ids.len(),
        "gift_wrap_ids": wrap_ids,
        "published": true,
        "title": title,
        "start": start,
        "end": end,
        "timezone": timezone
    }))
}

fn signer_nip44_encrypt(
    identity: &str,
    recipient_pubkey: &str,
    plaintext: &str,
) -> Result<String, String> {
    match send_request::<_, evermore_common::Response>(
        &evermore_common::runtime_socket_path(),
        &evermore_common::Request::Nip44Encrypt {
            identity: identity.into(),
            recipient_pubkey: recipient_pubkey.into(),
            plaintext: plaintext.into(),
        },
    )? {
        evermore_common::Response::Ok { data } => data
            .get("ciphertext")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or("signer response is missing ciphertext".into()),
        evermore_common::Response::Err { message } => Err(message),
    }
}

fn validate_calendar_mutation_arguments(arguments: &Value) -> Result<(), String> {
    for field in ["event_id", "expected_title", "title"] {
        arguments[field]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("calendar {field} is missing"))?;
    }
    EventId::parse(arguments["event_id"].as_str().unwrap())
        .map_err(|_| "calendar writes require the selected event's event_id")?;
    validate_calendar_destination(
        arguments["relay_url"]
            .as_str()
            .ok_or("calendar relay destination is missing")?,
    )
}

fn validate_calendar_delete_arguments(arguments: &Value) -> Result<(), String> {
    let event_id = arguments["event_id"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or("calendar event identifier is missing")?;
    if event_id.len() > 256 || event_id.contains(['\n', '\r', '\0']) {
        return Err("calendar event identifier is invalid".into());
    }
    validate_calendar_destination(
        arguments["relay_url"]
            .as_str()
            .ok_or("calendar relay destination is missing")?,
    )
}

fn validate_calendar_destination(relay_url: &str) -> Result<(), String> {
    if !(relay_url.starts_with("wss://")
        || relay_url.starts_with("ws://127.0.0.1:")
        || relay_url.starts_with("ws://localhost:")
        || relay_url.starts_with("ws://[::1]:"))
    {
        return Err("calendar relays must use wss://; unencrypted ws:// is allowed only for loopback testing".into());
    }
    if !std::env::var("EVERMORE_CALENDAR_RELAYS")
        .unwrap_or_default()
        .split([',', ';'])
        .map(str::trim)
        .any(|configured| configured == relay_url)
    {
        return Err("relay destination is not in EVERMORE_CALENDAR_RELAYS".into());
    }
    Ok(())
}

fn update_calendar_event(arguments: &Value, operation_id: &str) -> Result<Value, String> {
    let owner = OWNER_IDENTITY
        .get()
        .ok_or("owner identity is unavailable")?;
    let owner_key = load_public_key(owner)?;
    let (mut raw, private) = calendar::get_for_write(
        arguments["event_id"]
            .as_str()
            .ok_or("calendar event identifier is missing")?,
        owner,
        &owner_key,
    )?;
    let author = raw["pubkey"]
        .as_str()
        .ok_or("calendar event author is missing")?;
    if author != owner_key.to_hex() {
        return Err("only the event author can edit this calendar event".into());
    }
    let kind = raw["kind"]
        .as_u64()
        .ok_or("calendar event kind is missing")? as u16;
    if !matches!(kind, 31922 | 31923) {
        return Err("only calendar event kinds 31922 and 31923 can be edited".into());
    }
    let expected_title = arguments["expected_title"]
        .as_str()
        .ok_or("current event title is missing from the reviewed request")?;
    let current_title = raw["tags"]
        .as_array()
        .and_then(|tags| {
            tags.iter().find_map(|tag| {
                let tag = tag.as_array()?;
                (tag.first()?.as_str()? == "title")
                    .then(|| tag.get(1)?.as_str())
                    .flatten()
            })
        })
        .ok_or("calendar event title is missing")?;
    if expected_title != current_title {
        return Err(
            "calendar event changed since it was selected; review the latest event before editing"
                .into(),
        );
    }
    let prior_coordinate = event_coordinate(&raw)?;
    let mut tags = raw["tags"]
        .as_array()
        .ok_or("calendar event tags are missing")?
        .iter()
        .map(|tag| {
            tag.as_array()
                .ok_or("calendar event tag is malformed")?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or("calendar event tag value is malformed")
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let participants = tags
        .iter()
        .filter(|tag| tag.first().map(String::as_str) == Some("p"))
        .filter_map(|tag| tag.get(1))
        .map(|value| PublicKey::parse(value).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(requested) = arguments.get("participants").and_then(Value::as_array) {
        let requested = requested
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| "participant pubkey must be a string".to_string())
                    .and_then(|value| PublicKey::parse(value).map_err(|error| error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut current = participants
            .iter()
            .map(PublicKey::to_hex)
            .collect::<Vec<_>>();
        let mut requested = requested.iter().map(PublicKey::to_hex).collect::<Vec<_>>();
        current.sort();
        requested.sort();
        if private && current != requested {
            return Err("changing private event recipients requires a separate invitation and revocation flow".into());
        }
        if !private {
            tags.retain(|tag| tag.first().map(String::as_str) != Some("p"));
            for pubkey in requested {
                tags.push(vec!["p".into(), pubkey]);
            }
        }
    }
    let title = arguments["title"]
        .as_str()
        .ok_or("calendar event title is missing")?;
    let description = arguments
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| raw["content"].as_str().unwrap_or("").to_owned());
    set_calendar_tag(&mut tags, "title", Some(vec!["title".into(), title.into()]));
    if kind == 31923 {
        if arguments.get("date_start").is_some() || arguments.get("date_end").is_some() {
            return Err("date_start/date_end are only valid for kind-31922 events".into());
        }
        let start = arguments["start"]
            .as_u64()
            .ok_or("timed event start must be Unix seconds")?;
        let end = arguments["end"]
            .as_u64()
            .ok_or("timed event end must be Unix seconds")?;
        if end <= start || end.saturating_sub(start) > 366 * 86400 {
            return Err(
                "calendar event end must follow start and span no more than 366 days".into(),
            );
        }
        let timezone = arguments["timezone"]
            .as_str()
            .ok_or("timed event time zone is missing")?;
        if !valid_time_zone(timezone) {
            return Err("calendar time zone must name an installed IANA time zone".into());
        }
        set_calendar_tag(
            &mut tags,
            "start",
            Some(vec!["start".into(), start.to_string()]),
        );
        set_calendar_tag(&mut tags, "end", Some(vec!["end".into(), end.to_string()]));
        set_calendar_tag(
            &mut tags,
            "start_tzid",
            Some(vec!["start_tzid".into(), timezone.into()]),
        );
        set_calendar_tag(
            &mut tags,
            "end_tzid",
            Some(vec!["end_tzid".into(), timezone.into()]),
        );
        tags.retain(|tag| tag.first().map(String::as_str) != Some("D"));
        for day in start / 86400..=((end - 1) / 86400) {
            tags.push(vec!["D".into(), day.to_string()]);
        }
    } else {
        if arguments.get("start").is_some()
            || arguments.get("end").is_some()
            || arguments.get("timezone").is_some()
        {
            return Err("start/end/timezone are only valid for kind-31923 events".into());
        }
        let start = arguments["date_start"]
            .as_str()
            .ok_or("date_start is required for a kind-31922 event")?;
        let end = arguments.get("date_end").and_then(Value::as_str);
        if !calendar::valid_date(start)
            || end.is_some_and(|end| !calendar::valid_date(end))
            || end.is_some_and(|end| end <= start)
        {
            return Err("date_start/date_end must be valid YYYY-MM-DD dates, with end exclusive and after start".into());
        }
        set_calendar_tag(&mut tags, "start", Some(vec!["start".into(), start.into()]));
        set_calendar_tag(
            &mut tags,
            "end",
            end.map(|end| vec!["end".into(), end.into()]),
        );
        set_calendar_tag(&mut tags, "start_tzid", None);
        set_calendar_tag(&mut tags, "end_tzid", None);
        tags.retain(|tag| tag.first().map(String::as_str) != Some("D"));
    }
    if arguments
        .get("description")
        .and_then(Value::as_str)
        .is_some()
    {
        set_calendar_tag(
            &mut tags,
            "summary",
            (!description.is_empty()).then(|| vec!["summary".into(), description.clone()]),
        );
    }
    raw["tags"] = serde_json::to_value(&tags).map_err(|error| error.to_string())?;
    raw["content"] = Value::String(description.clone());
    let relay_url = arguments["relay_url"]
        .as_str()
        .ok_or("calendar relay destination is missing")?;
    if private {
        let mut builder = EventBuilder::new(Kind::from(kind), description.clone());
        for tag in &tags {
            builder = builder.tag(Tag::parse(tag.clone()).map_err(|error| error.to_string())?);
        }
        let mut rumor = builder.finalize_unsigned(owner_key);
        rumor.ensure_id();
        let event_id = rumor
            .id
            .as_ref()
            .ok_or("updated rumor is missing its event id")?
            .to_hex();
        publish_private_rumor(owner, owner_key, rumor, participants, relay_url)?;
        Ok(
            serde_json::json!({"event_id": event_id, "coordinate": prior_coordinate, "kind": kind, "visibility": "private", "relay_url": relay_url, "published": true, "operation_id": operation_id}),
        )
    } else {
        let signed = match send_request::<_, evermore_common::Response>(
            &evermore_common::runtime_socket_path(),
            &evermore_common::Request::SignEvent {
                identity: owner.clone(),
                kind,
                content: description,
                tags,
                created_at: None,
            },
        )? {
            evermore_common::Response::Ok { data } => data,
            evermore_common::Response::Err { message } => return Err(message),
        };
        let event_id = signed["id"]
            .as_str()
            .ok_or("signer response is missing event id")?
            .to_string();
        publish_event_to_relay(relay_url, &signed, "calendar relay")?;
        publish_event_to_relay(
            &configured_relay_url(),
            &signed,
            "local control-plane relay",
        )?;
        Ok(
            serde_json::json!({"event_id": event_id, "coordinate": prior_coordinate, "kind": kind, "visibility": "public", "relay_url": relay_url, "published": true, "operation_id": operation_id}),
        )
    }
}

fn delete_calendar_event(arguments: &Value, _operation_id: &str) -> Result<Value, String> {
    let owner = OWNER_IDENTITY
        .get()
        .ok_or("owner identity is unavailable")?;
    let owner_key = load_public_key(owner)?;
    let event_id = arguments["event_id"]
        .as_str()
        .ok_or("calendar event identifier is missing")?;
    let (raw, private) = calendar::get_for_write(event_id, owner, &owner_key)?;
    if raw["pubkey"].as_str() != Some(&owner_key.to_hex()) {
        return Err("only the event author can delete this calendar event".into());
    }
    let expected_title = arguments["expected_title"]
        .as_str()
        .ok_or("current event title is missing from the reviewed request")?;
    let current_title = raw["tags"]
        .as_array()
        .and_then(|tags| {
            tags.iter().find_map(|tag| {
                let tag = tag.as_array()?;
                (tag.first()?.as_str()? == "title")
                    .then(|| tag.get(1)?.as_str())
                    .flatten()
            })
        })
        .ok_or("calendar event title is missing")?;
    if expected_title != current_title {
        return Err(
            "calendar event changed since it was selected; review the latest event before deleting"
                .into(),
        );
    }
    let kind = raw["kind"]
        .as_u64()
        .ok_or("calendar event kind is missing")? as u16;
    if !matches!(kind, 31922 | 31923) {
        return Err("only calendar event kinds 31922 and 31923 can be deleted".into());
    }
    let coordinate = event_coordinate(&raw)?;
    let relay_url = arguments["relay_url"]
        .as_str()
        .ok_or("calendar relay destination is missing")?;
    if private {
        let recipients = raw["tags"]
            .as_array()
            .ok_or("calendar event tags are missing")?
            .iter()
            .filter_map(|tag| {
                let values = tag.as_array()?;
                (values.first()?.as_str()? == "p")
                    .then(|| values.get(1)?.as_str())
                    .flatten()
            })
            .map(|value| PublicKey::parse(value).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let event_id = raw["id"]
            .as_str()
            .ok_or("private calendar event id is missing")?;
        let tombstone_tags = vec![
            vec!["e".into(), event_id.into()],
            vec!["a".into(), coordinate.clone()],
            vec!["k".into(), kind.to_string()],
        ];
        let mut builder =
            EventBuilder::new(Kind::from(5), "Calendar event withdrawn by its author");
        for tag in tombstone_tags {
            builder = builder.tag(Tag::parse(tag).map_err(|error| error.to_string())?);
        }
        let tombstone = builder.finalize_unsigned(owner_key);
        publish_private_rumor(owner, owner_key, tombstone, recipients, relay_url)?;
    } else {
        let event_id = raw["id"].as_str().ok_or("calendar event id is missing")?;
        let tags = vec![
            vec!["e".into(), event_id.into()],
            vec!["a".into(), coordinate.clone()],
            vec!["k".into(), kind.to_string()],
        ];
        let signed = match send_request::<_, evermore_common::Response>(
            &evermore_common::runtime_socket_path(),
            &evermore_common::Request::SignEvent {
                identity: owner.clone(),
                kind: 5,
                content: String::new(),
                tags,
                created_at: None,
            },
        )? {
            evermore_common::Response::Ok { data } => data,
            evermore_common::Response::Err { message } => return Err(message),
        };
        publish_event_to_relay(relay_url, &signed, "calendar relay deletion request")?;
        publish_event_to_relay(
            &configured_relay_url(),
            &signed,
            "local control-plane deletion request",
        )?;
    }
    Ok(serde_json::json!({
        "event_id": raw["id"],
        "coordinate": coordinate,
        "visibility": if private { "private" } else { "public" },
        "relay_url": relay_url,
        "deletion_requested": !private,
        "private_withdrawal_sent": private
    }))
}

fn publish_private_rumor(
    owner: &str,
    owner_key: PublicKey,
    mut rumor: UnsignedEvent,
    mut recipients: Vec<PublicKey>,
    relay_url: &str,
) -> Result<Vec<String>, String> {
    rumor.ensure_id();
    recipients.push(owner_key);
    recipients.sort_by_key(PublicKey::to_hex);
    recipients.dedup();
    let rumor_json = rumor.as_json();
    let rumor_time = rumor.created_at.as_secs();
    let mut wrap_ids = Vec::new();
    for recipient in recipients {
        let ephemeral = Keys::generate();
        let randomness =
            u64::from_be_bytes(ephemeral.public_key().to_bytes()[..8].try_into().unwrap());
        let wrap_timestamp = unix_time().saturating_sub(randomness % 172_800);
        let seal_randomness = u64::from_be_bytes(
            rumor
                .id
                .as_ref()
                .ok_or("private rumor is missing its id")?
                .to_bytes()[..8]
                .try_into()
                .unwrap(),
        ) ^ randomness;
        let seal_timestamp = unix_time().saturating_sub(seal_randomness % 172_800);
        let encrypted = signer_nip44_encrypt(owner, &recipient.to_hex(), &rumor_json)?;
        let seal = match send_request::<_, evermore_common::Response>(
            &evermore_common::runtime_socket_path(),
            &evermore_common::Request::SignEvent {
                identity: owner.into(),
                kind: 13,
                content: encrypted,
                tags: Vec::new(),
                created_at: Some(seal_timestamp.min(rumor_time)),
            },
        )? {
            evermore_common::Response::Ok { data } => {
                Event::from_json(data.to_string()).map_err(|error| error.to_string())?
            }
            evermore_common::Response::Err { message } => return Err(message),
        };
        let ciphertext = nip44::encrypt(
            ephemeral.secret_key(),
            &recipient,
            seal.as_json(),
            nip44::Version::default(),
        )
        .map_err(|error| error.to_string())?;
        let wrap = EventBuilder::new(Kind::GiftWrap, ciphertext)
            .tag(Tag::public_key(recipient))
            .custom_created_at(Timestamp::from(wrap_timestamp.min(rumor_time)))
            .finalize(&ephemeral)
            .map_err(|error| error.to_string())?;
        let value = serde_json::to_value(&wrap).map_err(|error| error.to_string())?;
        publish_event_to_relay(relay_url, &value, "calendar relay")?;
        if recipient == owner_key {
            publish_event_to_relay(&configured_relay_url(), &value, "local control-plane relay")?;
        }
        wrap_ids.push(wrap.id.to_hex());
    }
    Ok(wrap_ids)
}

fn event_coordinate(raw: &Value) -> Result<String, String> {
    let kind = raw["kind"]
        .as_u64()
        .ok_or("calendar event kind is missing")?;
    let pubkey = raw["pubkey"]
        .as_str()
        .ok_or("calendar event author is missing")?;
    let d = raw["tags"]
        .as_array()
        .and_then(|tags| {
            tags.iter().find_map(|tag| {
                let tag = tag.as_array()?;
                (tag.first()?.as_str()? == "d")
                    .then(|| tag.get(1)?.as_str())
                    .flatten()
            })
        })
        .ok_or("calendar event d tag is missing")?;
    Ok(format!("{kind}:{pubkey}:{d}"))
}

fn set_calendar_tag(tags: &mut Vec<Vec<String>>, name: &str, replacement: Option<Vec<String>>) {
    tags.retain(|tag| tag.first().map(String::as_str) != Some(name));
    if let Some(replacement) = replacement {
        tags.push(replacement);
    }
}

fn validate_calendar_event_arguments(arguments: &Value) -> Result<(), String> {
    arguments["title"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or("calendar event title is missing")?;
    let start = arguments["start"]
        .as_u64()
        .ok_or("calendar event start must be Unix seconds")?;
    let end = arguments["end"]
        .as_u64()
        .ok_or("calendar event end must be Unix seconds")?;
    if end <= start || end.saturating_sub(start) > 366 * 86400 {
        return Err("calendar event end must follow start and span no more than 366 days".into());
    }
    if !matches!(arguments["visibility"].as_str(), Some("public" | "private")) {
        return Err("calendar visibility must be public or private".into());
    }
    let relay_url = arguments["relay_url"]
        .as_str()
        .ok_or("calendar relay destination is missing")?;
    if !(relay_url.starts_with("wss://")
        || relay_url.starts_with("ws://127.0.0.1:")
        || relay_url.starts_with("ws://localhost:")
        || relay_url.starts_with("ws://[::1]:"))
    {
        return Err("calendar relays must use wss://; unencrypted ws:// is allowed only for loopback testing".into());
    }
    if !std::env::var("EVERMORE_CALENDAR_RELAYS")
        .unwrap_or_default()
        .split([',', ';'])
        .map(str::trim)
        .any(|configured| configured == relay_url)
    {
        return Err("relay destination is not in EVERMORE_CALENDAR_RELAYS".into());
    }
    let timezone = arguments["timezone"]
        .as_str()
        .ok_or("calendar event time zone is missing")?;
    if !valid_time_zone(timezone) {
        return Err("calendar time zone must name an installed IANA time zone".into());
    }
    if let Some(coordinate) = arguments.get("calendar").and_then(Value::as_str)
        && !coordinate.is_empty()
    {
        let parts: Vec<_> = coordinate.splitn(3, ':').collect();
        if parts.len() != 3
            || parts[0] != "31924"
            || parts[2].is_empty()
            || PublicKey::parse(parts[1]).is_err()
            || coordinate.contains(['\n', '\r', '\0'])
        {
            return Err("calendar must be a valid kind-31924 address coordinate".into());
        }
    }
    if let Some(participants) = arguments.get("participants").and_then(Value::as_array) {
        for participant in participants {
            let pubkey = participant
                .as_str()
                .ok_or("participant pubkey must be a string")?;
            PublicKey::parse(pubkey)
                .map_err(|error| format!("invalid participant pubkey: {error}"))?;
        }
    }
    Ok(())
}

fn valid_time_zone(value: &str) -> bool {
    if value.is_empty()
        || value.starts_with('/')
        || !value.contains('/')
        || value.contains(['\\', '\n', '\r', '\0'])
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+'))
    {
        return false;
    }
    std::path::Path::new("/usr/share/zoneinfo")
        .join(value)
        .is_file()
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
        "calendar.list" => calendar::list(
            arguments
                .get("limit")
                .and_then(Value::as_u64)
                .map(|limit| limit as usize)
                .unwrap_or(30),
            OWNER_IDENTITY
                .get()
                .ok_or("owner identity is unavailable")?,
            &load_public_key(
                OWNER_IDENTITY
                    .get()
                    .ok_or("owner identity is unavailable")?,
            )?,
        )?,
        "calendar.get" => {
            let identifier = arguments["event_id"].as_str().expect("validated string");
            let owner = OWNER_IDENTITY
                .get()
                .ok_or("owner identity is unavailable")?;
            let pubkey = load_public_key(owner)?;
            let records = calendar::list(100, owner, &pubkey)?;
            records
                .as_array()
                .and_then(|items| {
                    items.iter().find(|item| {
                        item.get("event_id").and_then(Value::as_str) == Some(identifier)
                            || item.get("coordinate").and_then(Value::as_str) == Some(identifier)
                    })
                })
                .cloned()
                .ok_or_else(|| format!("calendar event '{identifier}' was not found"))?
        }
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
        "packages.list_installed" => serde_json::to_value(npackd_client::list_installed()?)
            .map_err(|error| error.to_string())?,
        "packages.list_available" => {
            npackd_client::call("Search", serde_json::json!({"query": ""}))?
        }
        "packages.resolve" => {
            let publisher = arguments["publisher"].as_str().expect("validated string");
            let name = arguments["name"].as_str().expect("validated string");
            npackd_client::call(
                "GetPackage",
                serde_json::json!({"package": format!("{publisher}/{name}"), "user": true}),
            )?
        }
        "packages.explain_dependencies" => {
            let publisher = arguments["publisher"].as_str().expect("validated string");
            let name = arguments["name"].as_str().expect("validated string");
            let installed = npackd_client::list_installed()?;
            let package = installed
                .iter()
                .find(|pkg| pkg.publisher == publisher && pkg.name == name)
                .ok_or_else(|| format!("package '{publisher}/{name}' is not installed"))?;
            let dependencies: Vec<Value> = package
                .dependencies
                .iter()
                .map(|dependency| {
                    let found = installed.iter().find(|candidate| {
                        candidate.name == dependency.name
                            && dependency
                                .publisher
                                .as_deref()
                                .is_none_or(|publisher| candidate.publisher == publisher)
                    });
                    serde_json::json!({
                        "name": dependency.name,
                        "publisher": dependency.publisher,
                        "requirement": dependency.requirement,
                        "installed": found.is_some(),
                        "installed_version": found.map(|candidate| candidate.version.as_str())
                    })
                })
                .collect();
            serde_json::json!({
                "publisher": package.publisher,
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
