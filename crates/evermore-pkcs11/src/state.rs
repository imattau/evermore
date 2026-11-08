//! Provider state: a single RSA-2048 keypair representing the one
//! virtual "token" this module exposes, plus the open-session table.
//!
//! Scope note: this first increment proves the PKCS#11 ABI/mechanics
//! work end-to-end against real `homed` with a real, locally-held RSA
//! keypair -- `C_Decrypt` does genuine RSA-OAEP-SHA1 math. It
//! deliberately does *not* yet gate that decrypt on a NIP-46 approval
//! round-trip; see `functions.rs`'s `C_Decrypt` for exactly where that
//! gate belongs once it's built (PROJECT_IDEA.md's Phase 5.5 tracks it
//! as the next concrete step, not assumed-done here). One keypair per
//! module instance for now, not yet wired to `evermore-signer`'s
//! per-identity `backing::Backing::Remote` pairing records.

use rsa::RsaPrivateKey;
use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const KEY_FILE_NAME: &str = "pkcs11-identity.der";
const DEFAULT_STATE_DIR: &str = "/var/lib/evermore";
const RSA_KEY_BITS: usize = 2048;

pub fn default_state_dir() -> PathBuf {
    PathBuf::from(DEFAULT_STATE_DIR)
}

/// Generates and persists the provider's RSA keypair if one doesn't
/// already exist. Idempotent, same as `evermore-signer`'s
/// `device_identity::bootstrap` -- plaintext key material in a
/// root-owned, 0600 file, same trust boundary as an SSH host key.
fn bootstrap_key(state_dir: &PathBuf) -> std::io::Result<RsaPrivateKey> {
    let path = state_dir.join(KEY_FILE_NAME);
    if let Ok(der) = std::fs::read(&path)
        && let Ok(key) = RsaPrivateKey::from_pkcs1_der(&der)
    {
        return Ok(key);
    }

    std::fs::create_dir_all(state_dir)?;
    std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))?;

    let mut rng = rand::thread_rng();
    let key = RsaPrivateKey::new(&mut rng, RSA_KEY_BITS)
        .map_err(|e| std::io::Error::other(format!("generating RSA key: {e}")))?;
    let der = key
        .to_pkcs1_der()
        .map_err(|e| std::io::Error::other(format!("encoding RSA key: {e}")))?;

    let tmp_path = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp_path)?;
    file.write_all(der.as_bytes())?;
    drop(file);
    std::fs::rename(&tmp_path, &path)?;

    Ok(key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OaepHash {
    Sha1,
    Sha256,
}

#[derive(Default)]
pub struct Session {
    pub logged_in: bool,
    pub decrypting: bool,
    /// The OAEP hash algorithm negotiated by `C_DecryptInit`'s mechanism
    /// parameter -- `homed` probes and picks SHA-1 or SHA-256 per
    /// token, so this can't be hardcoded (verified live: it chose
    /// SHA-256 against this module's own mechanism list).
    pub decrypt_hash: Option<OaepHash>,
    /// Object handles still to be returned by `C_FindObjects`, set by
    /// `C_FindObjectsInit` and drained in order.
    pub find_results: Vec<u64>,
}

pub struct Provider {
    pub key: RsaPrivateKey,
    pub sessions: HashMap<u64, Session>,
    pub next_session_handle: u64,
}

/// Fixed object handles: 1 = the public key, 2 = the private key. There
/// is exactly one of each for now (one virtual token, one keypair).
pub const PUBLIC_KEY_HANDLE: u64 = 1;
pub const PRIVATE_KEY_HANDLE: u64 = 2;

static PROVIDER: OnceLock<Mutex<Provider>> = OnceLock::new();

pub fn provider() -> &'static Mutex<Provider> {
    PROVIDER.get_or_init(|| {
        let state_dir = std::env::var("EVERMORE_PKCS11_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_state_dir());
        let key = bootstrap_key(&state_dir).expect("bootstrapping evermore-pkcs11 identity key");
        Mutex::new(Provider {
            key,
            sessions: HashMap::new(),
            next_session_handle: 1,
        })
    })
}
