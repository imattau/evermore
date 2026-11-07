//! The device-root identity: a machine-level Nostr keypair generated once
//! on first run and owned by the device itself, never by a human login
//! session (see PROJECT_IDEA.md's Phase 5.5). It is loaded by a *separate,
//! system-scoped* `evermore-signer serve --device` process -- a distinct
//! privilege/lifetime domain from the per-user daemon, the same way the
//! D-Bus system bus is distinct from the session bus -- so none of
//! `keyring_store.rs`'s existing session-keyring code needs to change: the
//! device process simply has its own session keyring, just like any other
//! process does.
//!
//! At-rest protection mirrors SSH host keys (`/etc/ssh/ssh_host_*_key`):
//! plaintext key material in a root-owned, 0600 file, with OS permissions
//! and disk encryption as the trust boundary. There is no human passphrase
//! to wrap this in at this stage, so adding a second encryption layer here
//! would be complexity with no real secret behind it.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use nostr::prelude::*;

/// Reserved identity name the device identity is loaded under in the
/// device signer process's session keyring. Never usable as a human
/// identity name (see `main.rs`'s `keygen`/`import` guard).
pub const RESERVED_IDENTITY_NAME: &str = "__device_root__";

const STATE_FILE_NAME: &str = "device-identity";
const DEFAULT_STATE_DIR: &str = "/var/lib/evermore";

pub fn default_state_dir() -> PathBuf {
    PathBuf::from(DEFAULT_STATE_DIR)
}

fn state_file(state_dir: &Path) -> PathBuf {
    state_dir.join(STATE_FILE_NAME)
}

/// Generates and persists a device identity if one doesn't already exist
/// under `state_dir`. Idempotent: if a device identity file is already
/// present, returns its existing public key unchanged rather than erroring
/// -- re-running this on every boot is expected to be a no-op after the
/// first.
pub fn bootstrap(state_dir: &Path) -> Result<PublicKey, String> {
    let path = state_file(state_dir);
    if path.exists() {
        return Ok(read_keys(&path)?.public_key());
    }

    std::fs::create_dir_all(state_dir)
        .map_err(|e| format!("creating {}: {e}", state_dir.display()))?;
    std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("setting permissions on {}: {e}", state_dir.display()))?;

    let keys = Keys::generate();

    // Write via a temp file + rename so a crash mid-write can never leave a
    // truncated/partial key on disk for a later boot to load.
    let tmp_path = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp_path)
        .map_err(|e| format!("creating {}: {e}", tmp_path.display()))?;
    file.write_all(keys.secret_key().to_secret_hex().as_bytes())
        .map_err(|e| format!("writing {}: {e}", tmp_path.display()))?;
    drop(file);
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("renaming {} into place: {e}", tmp_path.display()))?;

    Ok(keys.public_key())
}

/// Loads the device identity from disk into the *current process's*
/// session keyring under [`RESERVED_IDENTITY_NAME`], ready for `serve` to
/// pick up through the same `keyring_store` path human identities use.
pub fn load_into_keyring(state_dir: &Path) -> Result<(), String> {
    let path = state_file(state_dir);
    let keys = read_keys(&path)
        .map_err(|e| format!("{e} -- run `evermore-signer bootstrap-device` first"))?;
    crate::keyring_store::store(RESERVED_IDENTITY_NAME, keys.secret_key().as_secret_bytes())
        .map_err(|e| e.to_string())
}

fn read_keys(path: &Path) -> Result<Keys, String> {
    let hex =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let secret_key = SecretKey::parse(hex.trim()).map_err(|e| e.to_string())?;
    Ok(Keys::new(secret_key))
}
