//! Thin blocking client for evermore-signer's Unix socket. Good enough for
//! the Identity tab's "refresh" button; a real async client (and a daemon
//! connection kept open across the app's lifetime) can replace this once
//! more tabs need the daemon concurrently.

use evermore_common::{Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone)]
pub struct Identity {
    pub name: String,
    pub npub: String,
    pub pubkey_hex: String,
}

pub fn fetch_identities() -> Result<Vec<Identity>, String> {
    let mut stream = connect()?;
    let names: Vec<String> = match request(&mut stream, Request::ListIdentities)? {
        Response::Ok { data } => serde_json::from_value(data).map_err(|e| e.to_string())?,
        Response::Err { message } => return Err(message),
    };

    let mut identities = Vec::with_capacity(names.len());
    for name in names {
        let req = Request::GetPublicKey {
            identity: name.clone(),
        };
        match request(&mut stream, req)? {
            Response::Ok { data } => {
                let npub = data
                    .get("npub")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let pubkey_hex = data
                    .get("pubkey_hex")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                identities.push(Identity {
                    name,
                    npub,
                    pubkey_hex,
                });
            }
            Response::Err { message } => return Err(format!("{name}: {message}")),
        }
    }
    Ok(identities)
}

pub fn sign_event(
    identity: &str,
    kind: u16,
    content: String,
    tags: Vec<Vec<String>>,
) -> Result<serde_json::Value, String> {
    let sign_request = Request::SignEvent {
        identity: identity.to_string(),
        kind,
        content,
        tags,
        created_at: None,
    };
    let mut stream = connect()?;
    match request(&mut stream, sign_request)? {
        Response::Ok { data } => Ok(data),
        Response::Err { message } => Err(message),
    }
}

pub fn nip44_encrypt(
    identity: &str,
    recipient_pubkey: &str,
    plaintext: String,
) -> Result<String, String> {
    let mut stream = connect()?;
    match request(
        &mut stream,
        Request::Nip44Encrypt {
            identity: identity.to_string(),
            recipient_pubkey: recipient_pubkey.to_string(),
            plaintext,
        },
    )? {
        Response::Ok { data } => data
            .get("ciphertext")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "signer response omitted NIP-44 ciphertext".into()),
        Response::Err { message } => Err(message),
    }
}

fn connect() -> Result<UnixStream, String> {
    let path = evermore_common::runtime_socket_path();
    UnixStream::connect(&path).map_err(|e| {
        format!(
            "couldn't connect to evermore-signer at {} ({e}) -- is it running? (`evermore-signer serve`)",
            path.display()
        )
    })
}

fn request(stream: &mut UnixStream, req: Request) -> Result<Response, String> {
    let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&response_line).map_err(|e| e.to_string())
}
