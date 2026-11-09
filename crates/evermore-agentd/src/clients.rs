use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use evermore_common::operations::{self, Request, Response};
use evermore_common::{Request as SignerRequest, Response as SignerResponse};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct Grant {
    pub agent_pubkey: String,
    pub allowed_operations: Vec<String>,
    pub expires_at: u64,
    #[serde(default)]
    pub grant_published: bool,
    #[serde(default)]
    pub revoked: bool,
}

pub fn configured_owner() -> Result<String, String> {
    request_operations(&Request::ConfiguredOwner)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "operation daemon returned an invalid owner public key".into())
}

pub fn invoke(name: &str, arguments: &Value) -> Result<Value, String> {
    request_operations(&Request::Invoke {
        name: name.to_string(),
        arguments: arguments.clone(),
    })
}

pub fn grants() -> Result<Vec<Grant>, String> {
    let value = request_operations(&Request::ListGrants)?;
    serde_json::from_value(value).map_err(|error| format!("invalid grant list: {error}"))
}

pub fn require_grant(agent_pubkey: &str, required: &[String]) -> Result<(), String> {
    if required.is_empty() {
        return Ok(());
    }
    let grants = grants()?;
    let now = unix_time();
    for operation in required {
        let allowed = grants.iter().any(|grant| {
            grant.agent_pubkey == agent_pubkey
                && grant.grant_published
                && !grant.revoked
                && now < grant.expires_at
                && grant
                    .allowed_operations
                    .iter()
                    .any(|allowed| allowed == operation)
        });
        if !allowed {
            return Err(format!(
                "agent {agent_pubkey} has no active grant for '{operation}'"
            ));
        }
    }
    Ok(())
}

pub fn signer_public_key(identity: &str) -> Result<String, String> {
    let data = request_signer(&SignerRequest::GetPublicKey {
        identity: identity.to_string(),
    })?;
    data.get("pubkey_hex")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "signer returned an invalid public key".into())
}

fn request_operations(request: &Request) -> Result<Value, String> {
    request_socket(
        &operations::runtime_socket_path(),
        request,
        "operation daemon",
    )
}

fn request_signer(request: &SignerRequest) -> Result<Value, String> {
    request_socket(
        &evermore_common::runtime_socket_path(),
        request,
        "signer daemon",
    )
}

fn request_socket<T: serde::Serialize>(
    path: &std::path::Path,
    request: &T,
    label: &str,
) -> Result<Value, String> {
    let mut stream = UnixStream::connect(path).map_err(|error| {
        format!(
            "couldn't connect to the {label} at {}: {error}",
            path.display()
        )
    })?;
    let line = serde_json::to_string(request).map_err(|error| error.to_string())?;
    writeln!(stream, "{line}").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(|error| error.to_string())?;
    if response.trim().is_empty() {
        return Err(format!("{label} closed the connection without a response"));
    }
    match serde_json::from_str::<Response>(&response) {
        Ok(Response::Ok { data }) => Ok(data),
        Ok(Response::Err { message }) => Err(message),
        Err(_) => match serde_json::from_str::<SignerResponse>(&response) {
            Ok(SignerResponse::Ok { data }) => Ok(data),
            Ok(SignerResponse::Err { message }) => Err(message),
            Err(error) => Err(format!("{label} returned an invalid response: {error}")),
        },
    }
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
