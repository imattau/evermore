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

/// Wire protocol for `evermore-settingsd` and its clients. Same framing
/// (newline-delimited JSON) and the same `Ok`/`Err` response shape as the
/// signer's, kept in its own module since it's a different socket/daemon.
pub mod settings {
    use serde::{Deserialize, Serialize};

    pub const SOCKET_RELATIVE_PATH: &str = "evermore/settingsd.sock";

    pub fn runtime_socket_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        std::path::Path::new(&base).join(SOCKET_RELATIVE_PATH)
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        /// List the settings domains currently known (i.e. that have at
        /// least one key set).
        ListDomains,
        /// Get a single key's value within a domain.
        Get { domain: String, key: String },
        /// List every key/value pair in a domain.
        GetDomain { domain: String },
        /// Set a key's value within a domain. Triggers a NIP-44-encrypted
        /// kind:30078 event (one per domain, replacing the previous one)
        /// to be signed and published to the control-plane relay.
        Set {
            domain: String,
            key: String,
            value: serde_json::Value,
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
}

/// Wire protocol for `evermore-notifyd` and its clients (the panel's
/// Notification Center). Same framing as the other two sockets.
pub mod notifications {
    use serde::{Deserialize, Serialize};

    pub const SOCKET_RELATIVE_PATH: &str = "evermore/notifyd.sock";

    pub fn runtime_socket_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        std::path::Path::new(&base).join(SOCKET_RELATIVE_PATH)
    }

    /// Where a notification came from -- shown in the panel so the merged
    /// timeline (PROJECT_IDEA.md's Phase 3 goal) can be filtered by source.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Source {
        /// A local-relay event tagged ["t","evermore-notify"] -- a daemon
        /// on this machine announcing something (e.g. an update available).
        Local,
        /// A public mention/reply (kind:1 with a `p` tag for us) from an
        /// external relay.
        Mention,
        /// A NIP-17 gift-wrapped DM, unwrapped.
        Dm,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct NotificationRecord {
        pub source: Source,
        pub title: String,
        pub body: String,
        pub received_at: u64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        /// Most-recent-first, capped at whatever the daemon keeps in memory.
        ListRecent,
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
}
