//! Blocking client for evermore-signer's Unix socket. npack only signs
//! events (trust-list updates, package/release announcements, NIP-98
//! upload auth) -- it never decrypts anything.

use nostr::prelude::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
enum Request {
    GetPublicKey {
        identity: String,
    },
    SignEvent {
        identity: String,
        kind: u16,
        content: String,
        #[serde(default)]
        tags: Vec<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        created_at: Option<u64>,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok { data: serde_json::Value },
    Err { message: String },
}

pub struct SignerClient {
    identity: String,
}

impl SignerClient {
    pub fn new(identity: impl Into<String>) -> Self {
        Self {
            identity: identity.into(),
        }
    }

    pub fn get_public_key(&self) -> Result<String, String> {
        let data = self.call(Request::GetPublicKey {
            identity: self.identity.clone(),
        })?;
        data.get("pubkey_hex")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| "signer response missing pubkey_hex".to_string())
    }

    pub fn sign_event(
        &self,
        kind: u16,
        content: String,
        tags: Vec<Vec<String>>,
    ) -> Result<Event, String> {
        let data = self.call(Request::SignEvent {
            identity: self.identity.clone(),
            kind,
            content,
            tags,
            created_at: None,
        })?;
        serde_json::from_value(data).map_err(|e| e.to_string())
    }

    fn call(&self, req: Request) -> Result<serde_json::Value, String> {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let path = std::path::Path::new(&base).join("evermore/signer.sock");
        let mut stream = UnixStream::connect(&path).map_err(|e| {
            format!(
                "couldn't connect to evermore-signer at {} ({e}) -- is it running?",
                path.display()
            )
        })?;
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
        match serde_json::from_str::<Response>(&response_line).map_err(|e| e.to_string())? {
            Response::Ok { data } => Ok(data),
            Response::Err { message } => Err(message),
        }
    }
}
