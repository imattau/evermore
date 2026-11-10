use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

use evermore_common::{Request, Response};
use nostr::prelude::*;
use serde_json::json;

use crate::backing::{self, Backing};
use crate::identity_store;
use crate::keyring_store;
use zeroize::Zeroizing;

pub fn run(socket_path: std::path::PathBuf) -> std::io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    // A stale socket from a previous run (crashed, killed) would otherwise
    // make bind() fail with AddrInUse.
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

    eprintln!("evermore-signer listening on {}", socket_path.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                std::thread::spawn(move || {
                    if let Err(e) = handle_connection(stream) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
    Ok(())
}

fn handle_connection(stream: UnixStream) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle_request(req),
            Err(e) => Response::err(format!("invalid request: {e}")),
        };
        let mut out = serde_json::to_string(&response).unwrap_or_else(|_| {
            r#"{"status":"err","message":"failed to serialize response"}"#.to_string()
        });
        out.push('\n');
        writer.write_all(out.as_bytes())?;
        writer.flush()?;
    }
    Ok(())
}

/// Loads `identity`'s keys for signing/encryption, or `Ok(None)` if it
/// doesn't exist. An identity paired to a remote NIP-46 signer (see
/// `backing.rs`) never has a secret in this process's keyring at all --
/// that's the design, not a bug -- so this returns a clear, specific
/// error for one rather than falling through to "unknown identity" or
/// trying to read a secret that was never meant to exist here.
fn load_keys(identity: &str) -> Result<Option<Keys>, String> {
    if let Backing::Remote { .. } = backing::backing_for(identity) {
        return Err(format!(
            "identity '{identity}' is paired to a remote NIP-46 signer -- \
             remote signing is not implemented yet (see PROJECT_IDEA.md's Phase 5.5)"
        ));
    }
    let bytes = keyring_store::load(identity).map_err(|e| e.to_string())?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let secret_key = SecretKey::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(Some(Keys::new(secret_key)))
}

fn handle_request(req: Request) -> Response {
    match req {
        Request::ListIdentities => match keyring_store::list() {
            Ok(names) => Response::ok(names),
            Err(e) => Response::err(e.to_string()),
        },

        Request::CreateAgentIdentity { slug } => match create_agent_identity(&slug) {
            Ok(value) => Response::ok(value),
            Err(error) => Response::err(error),
        },

        Request::DeleteAgentIdentity { slug } => match delete_agent_identity(&slug) {
            Ok(value) => Response::ok(value),
            Err(error) => Response::err(error),
        },

        Request::GetPublicKey { identity } => match load_keys(&identity) {
            Ok(Some(keys)) => {
                let pk = keys.public_key();
                Response::ok(json!({
                    "pubkey_hex": pk.to_hex(),
                    "npub": pk.to_bech32().unwrap_or_default(),
                }))
            }
            Ok(None) => Response::err(format!("unknown identity '{identity}'")),
            Err(e) => Response::err(e),
        },

        Request::SignEvent {
            identity,
            kind,
            content,
            tags,
            created_at,
        } => match load_keys(&identity) {
            Ok(Some(keys)) => {
                let mut builder = EventBuilder::new(Kind::from(kind), content);
                for raw_tag in tags {
                    match Tag::parse(raw_tag) {
                        Ok(tag) => builder = builder.tag(tag),
                        Err(e) => return Response::err(format!("invalid tag: {e}")),
                    }
                }
                if let Some(ts) = created_at {
                    builder = builder.custom_created_at(Timestamp::from(ts));
                }
                match builder.finalize(&keys) {
                    Ok(event) => Response::ok(event),
                    Err(e) => Response::err(e.to_string()),
                }
            }
            Ok(None) => Response::err(format!("unknown identity '{identity}'")),
            Err(e) => Response::err(e),
        },

        Request::Nip44Encrypt {
            identity,
            recipient_pubkey,
            plaintext,
        } => match load_keys(&identity) {
            Ok(Some(keys)) => match PublicKey::parse(&recipient_pubkey) {
                Ok(pk) => match keys.nip44_encrypt(&pk, &plaintext) {
                    Ok(ciphertext) => Response::ok(json!({ "ciphertext": ciphertext })),
                    Err(e) => Response::err(e.to_string()),
                },
                Err(e) => Response::err(format!("invalid recipient_pubkey: {e}")),
            },
            Ok(None) => Response::err(format!("unknown identity '{identity}'")),
            Err(e) => Response::err(e),
        },

        Request::Nip44Decrypt {
            identity,
            sender_pubkey,
            ciphertext,
        } => match load_keys(&identity) {
            Ok(Some(keys)) => match PublicKey::parse(&sender_pubkey) {
                Ok(pk) => match keys.nip44_decrypt(&pk, &ciphertext) {
                    Ok(plaintext) => Response::ok(json!({ "plaintext": plaintext })),
                    Err(e) => Response::err(e.to_string()),
                },
                Err(e) => Response::err(format!("invalid sender_pubkey: {e}")),
            },
            Ok(None) => Response::err(format!("unknown identity '{identity}'")),
            Err(e) => Response::err(e),
        },
    }
}

/// Generates a new agent key, loads it into this session's keyring, and
/// persists it so the same identity is available after logout. A failure to
/// persist rolls the keyring entry back rather than leaving a key that looks
/// usable now but disappears at the next login.
fn create_agent_identity(slug: &str) -> Result<serde_json::Value, String> {
    let name = identity_store::identity_name(slug)?;
    if keyring_store::exists(&name).map_err(|error| error.to_string())? {
        return Err(format!("agent identity '{name}' is already loaded"));
    }
    if identity_store::exists(&name)? {
        return Err(format!("agent identity '{name}' already exists on disk"));
    }

    let keys = Keys::generate();
    let secret = Zeroizing::new(keys.secret_key().to_secret_hex());
    let pubkey = keys.public_key();
    keyring_store::store(&name, keys.secret_key().as_secret_bytes())
        .map_err(|error| error.to_string())?;
    if let Err(error) = identity_store::persist(&name, secret.as_str()) {
        let _ = keyring_store::remove(&name);
        return Err(error);
    }

    Ok(json!({
        "identity": name,
        "slug": slug,
        "pubkey_hex": pubkey.to_hex(),
        "npub": pubkey.to_bech32().unwrap_or_default(),
        "persisted": true,
    }))
}

fn delete_agent_identity(slug: &str) -> Result<serde_json::Value, String> {
    let name = identity_store::identity_name(slug)?;
    let loaded = keyring_store::exists(&name).map_err(|error| error.to_string())?;
    let persisted_removed = identity_store::remove(&name)?;
    if !loaded && !persisted_removed {
        return Err(format!("unknown agent identity '{name}'"));
    }
    keyring_store::remove(&name).map_err(|error| error.to_string())?;
    Ok(json!({
        "identity": name,
        "slug": slug,
        "deleted": true,
        "persisted_removed": persisted_removed,
    }))
}
