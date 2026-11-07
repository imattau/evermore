mod relay_client;
mod settings_store;
mod signer_client;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;

use evermore_common::settings::{Request, Response};
use nostr::prelude::*;
use relay_client::RelayClient;
use settings_store::{KIND_SETTINGS, SettingsStore, d_tag};
use signer_client::SignerClient;

const DEFAULT_RELAY_URL: &str = "ws://127.0.0.1:4869";

struct App {
    signer: SignerClient,
    relay: RelayClient,
    store: Arc<SettingsStore>,
    own_pubkey: String,
}

fn main() {
    let identity = match parse_identity_arg() {
        Ok(name) => name,
        Err(msg) => {
            eprintln!("{msg}");
            eprintln!("usage: evermore-settingsd --identity NAME [--relay ws://host:port]");
            std::process::exit(2);
        }
    };
    let relay_url = parse_relay_arg().unwrap_or_else(|| DEFAULT_RELAY_URL.to_string());

    let signer = SignerClient::new(identity);
    let own_pubkey = signer.get_public_key().unwrap_or_else(|e| {
        eprintln!("couldn't get public key from evermore-signer: {e}");
        std::process::exit(1);
    });
    eprintln!("evermore-settingsd: identity pubkey {own_pubkey}");

    let store = Arc::new(SettingsStore::new());

    let relay = {
        let store = Arc::clone(&store);
        let signer = signer.clone();
        let own_pubkey_for_decrypt = own_pubkey.clone();
        RelayClient::connect(&relay_url, own_pubkey.clone(), move |event| {
            on_settings_event(event, &signer, &store, &own_pubkey_for_decrypt)
        })
        .unwrap_or_else(|e| {
            eprintln!("couldn't connect to control-plane relay at {relay_url}: {e}");
            std::process::exit(1);
        })
    };
    eprintln!("evermore-settingsd: connected to relay at {relay_url}");

    let app = Arc::new(App {
        signer,
        relay,
        store,
        own_pubkey,
    });

    if let Err(e) = serve(app) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Decrypts an incoming kind:30078 event and updates the local cache. Runs
/// on the relay connection's own thread -- keep this fast, no socket I/O
/// back out to the signer beyond the one decrypt call.
fn on_settings_event(event: Event, signer: &SignerClient, store: &SettingsStore, own_pubkey: &str) {
    if event.kind != Kind::from(KIND_SETTINGS) || event.pubkey.to_hex() != own_pubkey {
        return;
    }
    let Some(domain) = event
        .tags
        .iter()
        .find(|t| t.as_slice().first().map(String::as_str) == Some("d"))
        .and_then(|t| t.as_slice().get(1))
        .and_then(|d| d.strip_prefix("evermore:"))
    else {
        return;
    };

    match signer.nip44_decrypt(own_pubkey, &event.content) {
        Ok(plaintext) => match serde_json::from_str::<serde_json::Value>(&plaintext) {
            Ok(serde_json::Value::Object(map)) => {
                store.replace_domain(domain, map);
                eprintln!("evermore-settingsd: updated domain '{domain}' from relay");
            }
            _ => {
                eprintln!("evermore-settingsd: settings event for '{domain}' wasn't a JSON object")
            }
        },
        Err(e) => eprintln!("evermore-settingsd: failed to decrypt settings event: {e}"),
    }
}

fn serve(app: Arc<App>) -> std::io::Result<()> {
    let socket_path = evermore_common::settings::runtime_socket_path();
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    eprintln!("evermore-settingsd listening on {}", socket_path.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let app = Arc::clone(&app);
                std::thread::spawn(move || {
                    if let Err(e) = handle_connection(stream, &app) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
    Ok(())
}

fn handle_connection(stream: UnixStream, app: &App) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle_request(req, app),
            Err(e) => Response::err(format!("invalid request: {e}")),
        };
        let mut out = serde_json::to_string(&response)
            .unwrap_or_else(|_| r#"{"status":"err","message":"serialization failed"}"#.to_string());
        out.push('\n');
        writer.write_all(out.as_bytes())?;
        writer.flush()?;
    }
    Ok(())
}

fn handle_request(req: Request, app: &App) -> Response {
    match req {
        Request::ListDomains => Response::ok(app.store.list_domains()),
        Request::Get { domain, key } => Response::ok(
            app.store
                .get(&domain, &key)
                .unwrap_or(serde_json::Value::Null),
        ),
        Request::GetDomain { domain } => Response::ok(app.store.get_domain(&domain)),
        Request::Set { domain, key, value } => set(app, &domain, &key, value),
    }
}

fn set(app: &App, domain: &str, key: &str, value: serde_json::Value) -> Response {
    let updated = app.store.set(domain, key, value);

    let plaintext = match serde_json::to_string(&updated) {
        Ok(s) => s,
        Err(e) => return Response::err(format!("failed to serialize domain: {e}")),
    };
    let ciphertext = match app.signer.nip44_encrypt(&app.own_pubkey, &plaintext) {
        Ok(c) => c,
        Err(e) => return Response::err(format!("encryption failed: {e}")),
    };
    let event = match app.signer.sign_event(
        KIND_SETTINGS,
        ciphertext,
        vec![vec!["d".to_string(), d_tag(domain)]],
    ) {
        Ok(e) => e,
        Err(e) => return Response::err(format!("signing failed: {e}")),
    };
    if let Err(e) = app.relay.publish(&event) {
        return Response::err(format!("publish failed: {e}"));
    }

    Response::ok(serde_json::json!({ "domain": domain, "updated": updated }))
}

fn parse_identity_arg() -> Result<String, String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == "--identity")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .ok_or_else(|| "missing required --identity NAME".to_string())
}

fn parse_relay_arg() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == "--relay")
        .and_then(|i| args.get(i + 1))
        .cloned()
}
