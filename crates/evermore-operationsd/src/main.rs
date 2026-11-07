mod store;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::time::Duration;

use evermore_common::operations::{self, Request, Response};
use nostr::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use store::{OperationStore, RelayPhase};
use tungstenite::Message;
use tungstenite::stream::MaybeTlsStream;

const DEFAULT_RELAY_URL: &str = "ws://127.0.0.1:4869";

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
        _ => {
            eprintln!(
                "usage: evermore-operationsd <serve|catalog|invoke|request|approve|grant|revoke-grant|grants|status>"
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
        .map_err(|error| format!("couldn't connect to control relay at {relay_url}: {error}"))?;
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
    if record.operation != "settings.set" {
        return store.finish_execution(
            operation_id,
            Err(format!(
                "no executor is registered for '{}'",
                record.operation
            )),
        );
    }
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
    operations::validate_input(&name, &arguments).map_err(|error| invalid_input(&error))?;
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
        .ok_or_else(|| invalid_input("decision must be approve or deny"))?;
    if decision != "approve" && decision != "deny" {
        return Err(invalid_input("decision must be approve or deny"));
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
    let response: Response = send_request(
        &operations::runtime_socket_path(),
        &Request::OperationStatus { operation_id },
    )
    .map_err(std::io::Error::other)?;
    print_response(response)
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
    operations::validate_input(name, arguments)?;
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
                evermore_common::notifications::Response::Ok { data } => data,
                evermore_common::notifications::Response::Err { message } => return Err(message),
            }
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
        _ => return Err(format!("'{name}' has no operation handler")),
    };

    Ok(operations::redact_result(&definition, data))
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
