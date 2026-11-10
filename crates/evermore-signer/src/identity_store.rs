//! On-disk persistence for agent and built-in service identities.
//!
//! Unlike the owner identity, these identities have to survive logout: an
//! agent's pubkey is bound into an owner-signed grant and referenced by
//! relay-backed agent definitions, and `operationsd`/`agentd` need their
//! service keys at every login. Secrets therefore live as 0600 regular files
//! under `$XDG_DATA_HOME/evermore/identities/` and are loaded into the session
//! keyring when the signer starts.
//!
//! User-creatable identities are confined to the `agent-` namespace. The
//! built-in `<owner>-operations` and `<owner>-assistant` identities share the
//! directory but are only provisioned by the idempotent
//! `ensure-service-identities` command. The owner key keeps its separate
//! `load-home-identity` path, and the device-root identity lives in
//! `device_identity`; neither can be reached through this module.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use nostr::prelude::SecretKey;
use zeroize::Zeroizing;

use crate::keyring_store;

/// The slug rules and identity naming are shared with `evermore-agentd`'s
/// relay projector, so both sides agree on what a valid agent id is.
pub use evermore_common::agent::agent_identity_name as identity_name;

/// Validates a name that may be persisted in the identity directory. This is
/// wider than an agent slug: the built-in service identities are
/// `<owner>-operations` and `<owner>-assistant`, so they share the same
/// directory but are not user-creatable agents.
pub fn validate_persisted_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("identity name must not be empty".to_string());
    }
    if name.len() > 64 {
        return Err("identity name must be at most 64 characters".to_string());
    }
    if !name.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
    }) {
        return Err(
            "identity name may only contain lowercase ASCII letters, digits, '-' or '_'"
                .to_string(),
        );
    }
    let first = name.as_bytes()[0];
    let last = name.as_bytes()[name.len() - 1];
    if !(first.is_ascii_alphanumeric() && last.is_ascii_alphanumeric()) {
        return Err("identity name must start and end with a letter or digit".to_string());
    }
    Ok(())
}

/// Names of the two built-in service identities derived from the owner's
/// login name. `operationsd` signs relay reconciliation events as
/// `<owner>-operations`; `agentd` runs the interactive assistant as
/// `<owner>-assistant`.
pub fn service_identity_names(owner: &str) -> Result<(String, String), String> {
    validate_persisted_name(owner)?;
    if owner == crate::device_identity::RESERVED_IDENTITY_NAME {
        return Err(format!(
            "'{owner}' is reserved for the device-root identity"
        ));
    }
    let operations = format!("{owner}-operations");
    let assistant = format!("{owner}-assistant");
    validate_persisted_name(&operations)?;
    validate_persisted_name(&assistant)?;
    Ok((operations, assistant))
}

/// `$XDG_DATA_HOME/evermore/identities`, falling back to
/// `$HOME/.local/share/evermore/identities`.
pub fn directory() -> Result<PathBuf, String> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())?;
            PathBuf::from(home).join(".local").join("share")
        }
    };
    Ok(base.join("evermore").join("identities"))
}

fn path_at(dir: &Path, name: &str) -> Result<PathBuf, String> {
    validate_persisted_name(name)?;
    Ok(dir.join(format!("{name}.nsec")))
}

fn ensure_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("creating {}: {error}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("setting permissions on {}: {error}", dir.display()))?;
    Ok(())
}

fn is_hex_secret(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Writes a 32-byte secret as a 0600 regular file. Refuses to overwrite any
/// existing path, including a symlink planted at the destination.
fn persist_at(dir: &Path, name: &str, secret_hex: &str) -> Result<PathBuf, String> {
    if !is_hex_secret(secret_hex) {
        return Err("agent secret must be 64 hexadecimal characters".to_string());
    }
    let path = path_at(dir, name)?;
    ensure_dir(dir)?;
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            return Err(format!(
                "agent identity '{}' already exists",
                path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("checking {}: {error}", path.display())),
    }

    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(secret_hex.as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&path);
        return Err(format!("writing {}: {error}", path.display()));
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("setting permissions on {}: {error}", path.display()))?;
    Ok(path)
}

fn load_at(dir: &Path, name: &str) -> Result<Option<Zeroizing<String>>, String> {
    let path = path_at(dir, name)?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("checking {}: {error}", path.display())),
    };
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be a regular file with no group or other permissions",
            path.display()
        ));
    }
    let bytes =
        std::fs::read(&path).map_err(|error| format!("reading {}: {error}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("{} is not UTF-8: {error}", path.display()))?;
    let secret = text.trim();
    if !is_hex_secret(secret) {
        return Err(format!(
            "{} does not hold a 32-byte hex secret",
            path.display()
        ));
    }
    Ok(Some(Zeroizing::new(secret.to_string())))
}

fn list_at(dir: &Path) -> Result<Vec<String>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("reading {}: {error}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(name) = file_name.strip_suffix(".nsec") else {
            continue;
        };
        if validate_persisted_name(name).is_err() {
            eprintln!("evermore-signer: ignoring unexpected identity file {file_name}");
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| format!("inspecting {}: {error}", entry.path().display()))?;
        if !file_type.is_file() {
            eprintln!(
                "evermore-signer: ignoring non-regular identity file {}",
                entry.path().display()
            );
            continue;
        }
        names.push(name.to_string());
    }
    names.sort();
    names.dedup();
    Ok(names)
}

pub fn persist(name: &str, secret_hex: &str) -> Result<PathBuf, String> {
    persist_at(&directory()?, name, secret_hex)
}

pub fn load(name: &str) -> Result<Option<Zeroizing<String>>, String> {
    load_at(&directory()?, name)
}

/// Whether a persisted secret file exists, without reading or validating its
/// contents.
pub fn exists(name: &str) -> Result<bool, String> {
    let path = path_at(&directory()?, name)?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("checking {}: {error}", path.display())),
    }
}

fn remove_at(dir: &Path, name: &str) -> Result<bool, String> {
    let path = path_at(dir, name)?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err(format!(
                    "refusing to delete non-regular identity file {}",
                    path.display()
                ));
            }
            std::fs::remove_file(&path)
                .map_err(|error| format!("deleting {}: {error}", path.display()))?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("checking {}: {error}", path.display())),
    }
}

pub fn remove(name: &str) -> Result<bool, String> {
    remove_at(&directory()?, name)
}

/// Loads every persisted agent identity into the session keyring. A single
/// malformed file is reported and skipped rather than preventing the signer
/// from starting; the other agents stay usable.
pub fn load_all_into_keyring() -> Result<Vec<String>, String> {
    let dir = directory()?;
    let mut loaded = Vec::new();
    for name in list_at(&dir)? {
        let secret = match load_at(&dir, &name) {
            Ok(Some(secret)) => secret,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("evermore-signer: skipping {name}: {error}");
                continue;
            }
        };
        let key = match SecretKey::parse(&secret) {
            Ok(key) => key,
            Err(error) => {
                eprintln!("evermore-signer: skipping {name}: invalid secret: {error}");
                continue;
            }
        };
        match keyring_store::store(&name, key.as_secret_bytes()) {
            Ok(()) => loaded.push(name),
            Err(error) => eprintln!("evermore-signer: couldn't load {name}: {error}"),
        }
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn slug_validation_rejects_paths_and_reserved_shapes() {
        for slug in ["morning-digest", "a", "package_watch_2", "x1"] {
            assert!(
                evermore_common::agent::validate_agent_slug(slug).is_ok(),
                "expected '{slug}' to be valid"
            );
        }
        for slug in [
            "",
            "Morning-Digest",
            "agent-morning",
            "../escape",
            "with/slash",
            ".hidden",
            "-leading",
            "trailing-",
            "with space",
        ] {
            assert!(
                evermore_common::agent::validate_agent_slug(slug).is_err(),
                "expected '{slug}' to be rejected"
            );
        }
    }

    #[test]
    fn persist_load_list_and_remove_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let name = "agent-morning-digest";

        let path = persist_at(dir.path(), name, SECRET).unwrap();
        assert_eq!(path, dir.path().join("agent-morning-digest.nsec"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(load_at(dir.path(), name).unwrap().unwrap().as_str(), SECRET);
        assert_eq!(list_at(dir.path()).unwrap(), vec![name.to_string()]);

        // A second create must not clobber an existing identity.
        assert!(persist_at(dir.path(), name, SECRET).is_err());

        assert!(remove_at(dir.path(), name).unwrap());
        assert!(load_at(dir.path(), name).unwrap().is_none());
        assert!(list_at(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn load_rejects_group_readable_files() {
        let dir = tempfile::tempdir().unwrap();
        let name = "agent-loose-permissions";
        let path = persist_at(dir.path(), name, SECRET).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(load_at(dir.path(), name).is_err());
    }

    #[test]
    fn list_ignores_unexpected_files() {
        let dir = tempfile::tempdir().unwrap();
        persist_at(dir.path(), "agent-one", SECRET).unwrap();
        persist_at(dir.path(), "alice-operations", SECRET).unwrap();
        std::fs::write(dir.path().join("Owner.nsec"), SECRET).unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a secret").unwrap();
        assert_eq!(
            list_at(dir.path()).unwrap(),
            vec!["agent-one".to_string(), "alice-operations".to_string()]
        );
    }

    #[test]
    fn service_identity_names_are_derived_and_validated() {
        let (operations, assistant) = service_identity_names("alice").unwrap();
        assert_eq!(operations, "alice-operations");
        assert_eq!(assistant, "alice-assistant");

        assert!(service_identity_names("Alice").is_err());
        assert!(service_identity_names("alice/../root").is_err());
        assert!(service_identity_names("__device_root__").is_err());
    }
}
