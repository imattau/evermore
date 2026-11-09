//! Identity storage backed by the Linux kernel keyring (session keyring).
//!
//! Each identity's 32-byte secp256k1 secret key is stored as a keyring key
//! under the description `evermore-signer:<name>`. The secret key bytes
//! never touch disk -- they live only in kernel memory for the lifetime of
//! the session keyring, exactly the property the signing daemon exists to
//! provide.
//!
//! **Scoping note:** the kernel's "session" keyring is tied to the PAM
//! login session, not the user account -- two separate SSH logins (or any
//! two processes not descended from the same login session) each get their
//! own, and keys added in one aren't visible from the other. This is a
//! non-issue for the real target (one graphical login session; the daemon,
//! the panel, and every app all share it) but trips up testing over
//! multiple separate `ssh` invocations -- `keygen` and `serve` (or any
//! client) need to run within the *same* session to see the same keys.

use linux_keyutils::{Key, KeyError, KeyRing, KeyRingIdentifier};

const PREFIX: &str = "evermore-signer:";

fn description_for(identity: &str) -> String {
    format!("{PREFIX}{identity}")
}

fn session_keyring() -> Result<KeyRing, KeyError> {
    KeyRing::from_special_id(KeyRingIdentifier::Session, false)
}

/// Stores `secret_key_bytes` (32 bytes) under `identity`, overwriting any
/// existing key with that description.
pub fn store(identity: &str, secret_key_bytes: &[u8]) -> Result<(), KeyError> {
    let ring = session_keyring()?;
    ring.add_key(&description_for(identity), secret_key_bytes)?;
    Ok(())
}

/// Loads the raw secret key bytes for `identity`, if present.
pub fn load(identity: &str) -> Result<Option<Vec<u8>>, KeyError> {
    let ring = session_keyring()?;
    match ring.search(&description_for(identity)) {
        Ok(key) => Ok(Some(key.read_to_vec()?)),
        Err(KeyError::KeyDoesNotExist) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Lists the identity names currently loaded in the session keyring.
pub fn list() -> Result<Vec<String>, KeyError> {
    let ring = session_keyring()?;
    let links = ring.get_links(256)?;
    let mut names = Vec::new();
    for link in links.iter() {
        let Some(key) = link.as_key() else { continue };
        let Ok(meta) = key.metadata() else { continue };
        if let Some(name) = meta.get_description().strip_prefix(PREFIX) {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// Returns true if an identity with this name already exists.
pub fn exists(identity: &str) -> Result<bool, KeyError> {
    Ok(load(identity)?.is_some())
}

/// Unlinks and revokes an identity's secret from the current session keyring.
pub fn remove(identity: &str) -> Result<(), KeyError> {
    let ring = session_keyring()?;
    match ring.search(&description_for(identity)) {
        Ok(key) => {
            key.revoke()?;
            ring.unlink_key(key)?;
            Ok(())
        }
        Err(KeyError::KeyDoesNotExist) => Ok(()),
        Err(error) => Err(error),
    }
}

#[allow(dead_code)]
pub fn key_object(identity: &str) -> Result<Option<Key>, KeyError> {
    let ring = session_keyring()?;
    match ring.search(&description_for(identity)) {
        Ok(key) => Ok(Some(key)),
        Err(KeyError::KeyDoesNotExist) => Ok(None),
        Err(e) => Err(e),
    }
}
