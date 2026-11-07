//! Shared types and helpers for `evermore-npack` -- kept in this crate's
//! own lib.rs rather than evermore-common, since npack doesn't need a
//! long-lived daemon/socket the way settings/notifications/operations do
//! (see main.rs's doc comment for why), so there's no other process that
//! needs to share these types over IPC.

pub mod archive;
pub mod blossom_client;
pub mod relay_client;
pub mod signer_client;

use serde::{Deserialize, Serialize};

/// Everything a `.npk` archive carries about itself, stored as
/// `manifest.json` at the archive root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Reverse-DNS-style package identifier, e.g. "sh.evermore.hello" --
    /// also used as the `d` tag for this package's kind:32267/30063 events.
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Path within the archive to symlink into `~/.local/bin/<name>` on
    /// install, if this package provides an executable.
    #[serde(default)]
    pub bin: Option<String>,
}

/// A discovered package: the latest kind:32267 (software-application)
/// event for some `id`, from a trusted publisher.
#[derive(Debug, Clone, Serialize)]
pub struct PackageListing {
    pub id: String,
    pub name: String,
    pub description: String,
    pub publisher_pubkey: String,
}

/// A discovered release: the latest kind:30063 (release-artifact set) for
/// some package `id` and `version`, from a trusted publisher.
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseListing {
    pub id: String,
    pub version: String,
    pub publisher_pubkey: String,
    pub blossom_url: String,
    pub sha256: String,
}

pub const KIND_SOFTWARE_APPLICATION: u16 = 32267;
pub const KIND_RELEASE_ARTIFACT_SET: u16 = 30063;
/// NIP-51 categorized people list (already on the relay's allowlist from
/// Phase 2) -- reused as-is for the trust list, no new kind needed for it.
pub const KIND_TRUSTED_PUBLISHERS: u16 = 30000;
pub const TRUSTED_PUBLISHERS_D_TAG: &str = "evermore:trusted-publishers";

pub fn install_root() -> std::path::PathBuf {
    let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/.local/share")
    });
    std::path::PathBuf::from(base)
        .join("evermore")
        .join("npack")
}

pub fn bin_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join(".local").join("bin")
}
