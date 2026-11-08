//! Reads the pre-login profile cache `evermore-signer set-profile`
//! writes (see `crates/evermore-signer/src/profile.rs`). Deliberately
//! does not touch the local control-plane relay at all: it's a
//! per-user systemd *user* service that only exists once a login
//! session has already started, so there is nothing for this,
//! pre-login, dedicated `greeter`-user process to query.

use std::path::PathBuf;

const PROFILES_DIR: &str = "/var/lib/evermore-profiles";

#[derive(serde::Deserialize)]
struct ProfileCache {
    name: String,
    avatar: Option<String>,
}

pub struct Profile {
    pub name: String,
    pub avatar_path: Option<PathBuf>,
}

/// Returns `None` if `pubkey_hex` has no cached profile at all (never
/// ran `set-profile`) rather than erroring -- a provisioned-but-blank
/// profile is an expected, normal state, not a fault.
pub fn load(pubkey_hex: &str) -> Option<Profile> {
    let dir = PathBuf::from(PROFILES_DIR).join(pubkey_hex);
    let raw = std::fs::read_to_string(dir.join("profile.json")).ok()?;
    let cache: ProfileCache = serde_json::from_str(&raw).ok()?;
    Some(Profile {
        name: cache.name,
        avatar_path: cache.avatar.map(|f| dir.join(f)),
    })
}
