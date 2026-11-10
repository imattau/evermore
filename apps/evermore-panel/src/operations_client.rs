use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use evermore_common::operations::{self, Definition, Request, Response};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Deserialize)]
pub struct PendingOperation {
    #[serde(default)]
    pub owner_pubkey: Option<String>,
    pub operation_id: String,
    pub actor_pubkey: String,
    pub operation: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub step_index: Option<u32>,
    pub arguments: Value,
    pub created_at: u64,
    pub expires_at: u64,
    pub request_published: bool,
    #[serde(default)]
    pub status_source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OperationHistoryEntry {
    pub operation_id: String,
    pub actor_pubkey: String,
    pub operation: String,
    pub operation_version: u32,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub step_index: Option<u32>,
    pub arguments: Value,
    pub status: String,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub created_at: u64,
    pub request_published: bool,
    pub approval_event_id: Option<String>,
    pub approval_published: bool,
    pub execution_event_id: Option<String>,
    #[serde(default)]
    pub execution_published: bool,
    pub result_event_id: Option<String>,
    pub result_published: bool,
    #[serde(default)]
    pub status_source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentGrant {
    #[serde(default)]
    pub grant_id: String,
    pub agent_pubkey: String,
    pub allowed_operations: Vec<String>,
    pub expires_at: u64,
    #[serde(default)]
    pub grant_published: bool,
    pub revoked: bool,
}

pub fn submit_grant(
    owner_identity: &str,
    agent_pubkey: &str,
    allowed_operations: &[String],
    expires_in_secs: u64,
) -> Result<AgentGrant, String> {
    if allowed_operations.is_empty() {
        return Err("an agent grant must allow at least one operation".into());
    }
    if expires_in_secs == 0 || expires_in_secs > 365 * 24 * 60 * 60 {
        return Err("grant expiry must be between 1 second and 365 days".into());
    }
    let expires_at = unix_time().saturating_add(expires_in_secs);
    let content = json!({
        "agent_pubkey": agent_pubkey,
        "allowed_operations": allowed_operations,
        "expires_at": expires_at,
    })
    .to_string();
    let event = crate::signer_client::sign_event(
        owner_identity,
        1,
        content,
        vec![
            vec!["t".into(), "evermore-agent-grant".into()],
            vec!["p".into(), agent_pubkey.into()],
        ],
    )?;
    let value = request(&Request::SubmitGrant { event })?;
    serde_json::from_value(value).map_err(|error| format!("invalid grant response: {error}"))
}

pub fn revoke_grant(owner_identity: &str, grant_id: &str) -> Result<AgentGrant, String> {
    let content = json!({ "grant_id": grant_id }).to_string();
    let event = crate::signer_client::sign_event(
        owner_identity,
        1,
        content,
        vec![
            vec!["t".into(), "evermore-agent-grant-revoke".into()],
            vec!["e".into(), grant_id.into()],
        ],
    )?;
    let value = request(&Request::RevokeGrant { event })?;
    serde_json::from_value(value).map_err(|error| format!("invalid grant response: {error}"))
}

pub fn configured_owner_pubkey() -> Result<String, String> {
    request(&Request::ConfiguredOwner)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "operation daemon returned an invalid owner public key".into())
}

pub fn fetch_catalog() -> Result<Vec<Definition>, String> {
    let socket_path = operations::runtime_socket_path();
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("couldn't connect to {}: {e}", socket_path.display()))?;
    let request = serde_json::to_string(&Request::Catalog).map_err(|e| e.to_string())?;
    writeln!(stream, "{request}").map_err(|e| e.to_string())?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let response: Response = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    match response {
        Response::Ok { data } => serde_json::from_value(data).map_err(|e| e.to_string()),
        Response::Err { message } => Err(message),
    }
}

pub fn fetch_grants() -> Result<Vec<AgentGrant>, String> {
    match request(&Request::ListGrants)? {
        Value::Array(grants) => {
            serde_json::from_value(Value::Array(grants)).map_err(|e| e.to_string())
        }
        _ => Err("operation daemon returned an invalid grant list".into()),
    }
}

pub fn pending_operations() -> Result<Vec<PendingOperation>, String> {
    match request(&Request::ListPendingOperations) {
        Err(_) => serde_json::from_value(Value::Array(operations::read_local_pending()?))
            .map_err(|e| e.to_string()),
        Ok(Value::Array(records)) => {
            serde_json::from_value(Value::Array(records)).map_err(|e| e.to_string())
        }
        Ok(_) => Err("operation daemon returned an invalid pending list".into()),
    }
}

pub fn history(limit: usize) -> Result<Vec<OperationHistoryEntry>, String> {
    match request(&Request::OperationHistory { limit }) {
        Err(_) => {
            let records = operations::read_local_history(limit)?;
            let records = records.into_iter().map(history_view).collect();
            serde_json::from_value(Value::Array(records)).map_err(|e| e.to_string())
        }
        Ok(Value::Array(records)) => {
            serde_json::from_value(Value::Array(records)).map_err(|e| e.to_string())
        }
        Ok(_) => Err("operation daemon returned an invalid history list".into()),
    }
}

fn history_view(mut record: Value) -> Value {
    for (field, event_field) in [
        ("approval_event_id", "approval_event"),
        ("execution_event_id", "execution_event"),
        ("result_event_id", "result_event"),
    ] {
        record[field] = record
            .get(event_field)
            .and_then(|event| event.get("id"))
            .cloned()
            .unwrap_or(Value::Null);
    }
    record
}

pub fn decide(identity: &str, operation_id: &str, decision: &str) -> Result<String, String> {
    if !["approve", "deny", "cancel"].contains(&decision) {
        return Err("decision must be approve, deny, or cancel".into());
    }
    let content = json!({ "operation_id": operation_id, "decision": decision }).to_string();
    let event = crate::signer_client::sign_event(
        identity,
        1,
        content,
        vec![
            vec!["t".into(), "evermore-operation-approval".into()],
            vec!["e".into(), operation_id.into()],
        ],
    )?;
    let result = request(&Request::SubmitApproval { event })?;
    result
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "approval response is missing status".into())
}

pub fn submit(
    identity: &str,
    owner_pubkey: &str,
    operation: &str,
    arguments: Value,
) -> Result<String, String> {
    let arguments = operations::canonicalize_input(operation, &arguments)?;
    let definition = operations::find(operation).ok_or("unknown operation")?;
    if definition.access != operations::Access::Write
        || definition.approval != operations::Approval::Required
    {
        return Err("only approval-gated writes can be proposed here".into());
    }
    let plaintext = json!({"operation": operation, "arguments": arguments}).to_string();
    let content = crate::signer_client::nip44_encrypt(identity, owner_pubkey, plaintext)?;
    let event = crate::signer_client::sign_event(
        identity,
        1,
        content,
        vec![
            vec!["t".into(), "evermore-operation-request".into()],
            vec!["p".into(), owner_pubkey.into()],
        ],
    )?;
    let result = request(&Request::SubmitSignedRequest { event })?;
    result
        .get("operation_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "operation daemon response omitted operation_id".into())
}

fn request(request: &Request) -> Result<Value, String> {
    let socket_path = operations::runtime_socket_path();
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("couldn't connect to {}: {e}", socket_path.display()))?;
    let line = serde_json::to_string(request).map_err(|e| e.to_string())?;
    writeln!(stream, "{line}").map_err(|e| e.to_string())?;
    let mut response_line = String::new();
    BufReader::new(stream)
        .read_line(&mut response_line)
        .map_err(|e| e.to_string())?;
    match serde_json::from_str::<Response>(&response_line).map_err(|e| e.to_string())? {
        Response::Ok { data } => Ok(data),
        Response::Err { message } => Err(message),
    }
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
