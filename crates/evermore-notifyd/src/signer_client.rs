//! Blocking client for evermore-signer's Unix socket. notifyd only ever
//! needs `get_public_key` (to know what to subscribe for) and
//! `nip44_decrypt` (to unwrap gift-wrapped DMs) -- it never signs anything.

use evermore_common::{Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

#[derive(Clone)]
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

    pub fn nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String, String> {
        let data = self.call(Request::Nip44Decrypt {
            identity: self.identity.clone(),
            sender_pubkey: sender_pubkey.to_string(),
            ciphertext: ciphertext.to_string(),
        })?;
        data.get("plaintext")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| "signer response missing plaintext".to_string())
    }

    fn call(&self, req: Request) -> Result<serde_json::Value, String> {
        let path = evermore_common::runtime_socket_path();
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
