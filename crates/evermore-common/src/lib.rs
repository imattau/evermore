//! Wire protocol shared between `evermore-signer` and its clients.
//!
//! The socket speaks newline-delimited JSON: one [`Request`] per line in,
//! one [`Response`] per line out. Kept free of any `nostr`-crate types so
//! clients don't have to pull in signing/crypto code just to talk to the
//! daemon -- event payloads cross the wire as plain JSON.

use serde::{Deserialize, Serialize};

/// Path (relative to `$XDG_RUNTIME_DIR`) of the signer's Unix socket.
pub const SOCKET_RELATIVE_PATH: &str = "evermore/signer.sock";

/// Resolves the signer socket path under `$XDG_RUNTIME_DIR` (falling back to
/// `/tmp` if unset, mainly so this doesn't panic outside a real session).
pub fn runtime_socket_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::Path::new(&base).join(SOCKET_RELATIVE_PATH)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    /// List the identity names currently loaded in the daemon.
    ListIdentities,
    /// Get an identity's public key (hex and bech32/npub).
    GetPublicKey { identity: String },
    /// Build and sign a Nostr event under the given identity, returning the
    /// full signed event as JSON (id, pubkey, sig, etc. all filled in).
    SignEvent {
        identity: String,
        kind: u16,
        content: String,
        #[serde(default)]
        tags: Vec<Vec<String>>,
        /// Unix timestamp; defaults to "now" if omitted.
        #[serde(skip_serializing_if = "Option::is_none")]
        created_at: Option<u64>,
    },
    /// NIP-44 encrypt `plaintext` from `identity` to `recipient_pubkey` (hex).
    Nip44Encrypt {
        identity: String,
        recipient_pubkey: String,
        plaintext: String,
    },
    /// NIP-44 decrypt `ciphertext` received by `identity` from `sender_pubkey` (hex).
    Nip44Decrypt {
        identity: String,
        sender_pubkey: String,
        ciphertext: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { data: serde_json::Value },
    Err { message: String },
}

impl Response {
    pub fn ok(data: impl Serialize) -> Self {
        Response::Ok {
            data: serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Response::Err {
            message: message.into(),
        }
    }
}
