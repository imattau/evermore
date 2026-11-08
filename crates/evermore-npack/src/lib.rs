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
    /// Path within the archive to a `systemd --user` unit file, if this
    /// package runs as a background daemon. Installed to
    /// `systemd_user_unit_dir()` and `enable --now`'d on install,
    /// `restart`'d on update -- the unit's own `Exec*=` lines point at the
    /// `bin` symlink above, so they don't need rewriting per version.
    #[serde(default)]
    pub systemd_unit: Option<String>,
    /// Optional signed local-model payload. The referenced GGUF is part of the
    /// archive and its digest is covered transitively by the signed release.
    #[serde(default)]
    pub ai_model: Option<AiModelPayload>,
    /// Direct package IDs required by this package. Resolution is reported
    /// from the user's installed package set; the manifest never executes a
    /// package-provided install command.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiModelPayload {
    /// Base GGUF file, relative to the package root.
    pub gguf_file: String,
    pub sha256: String,
    /// Optional LoRA adapter GGUF shipped in the same package.
    #[serde(default)]
    pub adapter_gguf_file: Option<String>,
    #[serde(default)]
    pub adapter_sha256: Option<String>,
    /// Default adapter scale when the package is imported.
    #[serde(default)]
    pub adapter_scale: Option<f32>,
    /// Preferred prompt context; `ai-inferd` clamps this to 512..=131072.
    #[serde(default)]
    pub context_size: Option<u32>,
    /// Quantization label for display, e.g. `Q4_K_M` or `F16`.
    #[serde(default)]
    pub quant: Option<String>,
    /// `chat` and/or `embeddings`; defaults to `chat` when absent.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Operation-registry version the adapter was trained against. Required
    /// when an adapter is shipped.
    #[serde(default)]
    pub operation_registry_version: Option<String>,
    /// Training provenance for a LoRA adapter.
    #[serde(default)]
    pub training: Option<evermore_common::ai::TrainingProvenance>,
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

/// Where a package's `systemd_unit` gets installed -- the standard
/// per-user unit directory `systemctl --user` already searches, so no
/// `--unit-path` wiring is needed on top of it.
pub fn systemd_user_unit_dir() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/.config")
    });
    std::path::PathBuf::from(base).join("systemd").join("user")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_payload_parses_base_and_adapter_fields() {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "id": "evermore.example.model",
            "name": "Example",
            "version": "1.0.0",
            "ai_model": {
                "gguf_file": "base.gguf",
                "sha256": "a".repeat(64),
                "adapter_gguf_file": "adapter.gguf",
                "adapter_sha256": "b".repeat(64),
                "adapter_scale": 0.75,
                "context_size": 8192,
                "quant": "Q4_K_M",
                "capabilities": ["chat"],
                "operation_registry_version": "2026-11-08"
            }
        }))
        .unwrap();
        let payload = manifest.ai_model.unwrap();
        assert_eq!(payload.adapter_scale, Some(0.75));
        assert_eq!(payload.capabilities, vec!["chat"]);
        assert_eq!(
            payload.operation_registry_version.as_deref(),
            Some("2026-11-08")
        );
    }

    #[test]
    fn legacy_model_payload_still_parses() {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "id": "legacy.model",
            "name": "Legacy",
            "version": "1.0.0",
            "ai_model": { "gguf_file": "model.gguf", "sha256": "c".repeat(64) }
        }))
        .unwrap();
        let payload = manifest.ai_model.unwrap();
        assert!(payload.adapter_gguf_file.is_none());
        assert!(payload.capabilities.is_empty());
        assert!(payload.context_size.is_none());
    }
}
