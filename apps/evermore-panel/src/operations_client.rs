use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use evermore_common::operations::{self, Definition, Request, Response};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Deserialize)]
pub struct PendingOperation {
    pub operation_id: String,
    pub actor_pubkey: String,
    pub operation: String,
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
