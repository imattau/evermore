//! Per-identity backing: whether an identity's secret lives locally (in
//! this process's session keyring, as every identity does today) or
//! remotely, behind a NIP-46 ("Nostr Connect") signer such as a paired
//! phone -- Phase 5.5's priority-1, passphrase-free unlock path. An
//! identity with no explicit record here is `Local` by default, so every
//! identity created before this concept existed keeps working exactly as
//! it did.
//!
//! This module only defines the fork point, not the NIP-46 protocol
//! itself. Connecting to a relay, constructing/encrypting a signed
//! request, and matching it to the right response is real, separate
//! protocol-level work that doesn't exist yet -- see PROJECT_IDEA.md's
//! Phase 5.5 for why (homed has no native slot for it; the concrete path
//! is a PKCS#11 provider that proxies to this). `server.rs` checks an
//! identity's backing and returns a clear, honest "not implemented yet"
//! for `Remote` rather than silently trying (and failing) to load a
//! secret that was never meant to exist on this device.
//!
//! Stored as plain JSON, not in the kernel keyring: a `Remote` record
//! holds no secret at all (that's the entire point -- the `nsec` never
//! touches this device), and it needs to persist across reboots, which
//! the session keyring does not.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Backing {
    Local,
    Remote {
        relay: String,
        remote_signer_pubkey: String,
    },
}

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/.config")
    });
    PathBuf::from(base)
        .join("evermore")
        .join("signer-identities.json")
}

fn load_all() -> HashMap<String, Backing> {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_all(map: &HashMap<String, Backing>) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// Returns `identity`'s backing. An identity with no explicit record is
/// `Local` -- the default that preserves every identity created before
/// this concept existed, with zero migration needed.
pub fn backing_for(identity: &str) -> Backing {
    load_all().remove(identity).unwrap_or(Backing::Local)
}

/// Registers `identity` as NIP-46-backed: no secret for it will ever be
/// stored in this process's keyring, and `keygen`/`import` refuse the
/// name afterward (see `main.rs`). Overwrites any existing record for the
/// same name.
pub fn pair_remote(identity: &str, relay: &str, remote_signer_pubkey: &str) -> Result<(), String> {
    let mut map = load_all();
    map.insert(
        identity.to_string(),
        Backing::Remote {
            relay: relay.to_string(),
            remote_signer_pubkey: remote_signer_pubkey.to_string(),
        },
    );
    save_all(&map)
}

/// Lists every identity with an explicit backing record on file (i.e.
/// every remote-paired identity recorded so far).
pub fn list_registered() -> HashMap<String, Backing> {
    load_all()
}
