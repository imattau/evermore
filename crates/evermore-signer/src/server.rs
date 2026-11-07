use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

use evermore_common::{Request, Response};
use nostr::prelude::*;
use serde_json::json;

use crate::keyring_store;

pub fn run() -> std::io::Result<()> {
    let socket_path = evermore_common::runtime_socket_path();
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

fn load_keys(identity: &str) -> Result<Option<Keys>, String> {
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
