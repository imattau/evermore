mod gift_wrap;
mod notification_store;
mod relay_client;
mod settings_client;
mod signer_client;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use evermore_common::notifications::{NotificationRecord, Request, Response, Source};
use nostr::prelude::*;
use notification_store::NotificationStore;
use notify_rust::Notification;
use signer_client::SignerClient;

const DEFAULT_LOCAL_RELAY_URL: &str = "ws://127.0.0.1:4869";

fn main() {
    let identity = match parse_arg("--identity") {
        Some(name) => name,
        None => {
            eprintln!("missing required --identity NAME");
            eprintln!("usage: evermore-notifyd --identity NAME [--local-relay ws://host:port]");
            std::process::exit(2);
        }
    };
    let local_relay_url =
        parse_arg("--local-relay").unwrap_or_else(|| DEFAULT_LOCAL_RELAY_URL.to_string());

    let signer = SignerClient::new(identity);
    let own_pubkey = signer.get_public_key().unwrap_or_else(|e| {
        eprintln!("couldn't get public key from evermore-signer: {e}");
        std::process::exit(1);
    });
    eprintln!("evermore-notifyd: identity pubkey {own_pubkey}");

    let store = Arc::new(NotificationStore::new());

    // Local control-plane relay: any kind:1 event tagged ["t","evermore-notify"]
    // is a daemon-on-this-machine notification, no decryption needed.
    {
        let store = Arc::clone(&store);
        let filters = serde_json::json!({"kinds": [1], "#t": ["evermore-notify"]});
        relay_client::spawn_subscription(
            "local",
            local_relay_url,
            filters,
            move |_label, event| {
                handle_local_event(event, &store);
            },
        );
    }

    // External relays (optional -- only if evermore-settingsd has an
    // `evermore:relays` / `external` list configured; see settings_client).
    let external_relays = settings_client::external_relay_urls();
    if external_relays.is_empty() {
        eprintln!(
            "evermore-notifyd: no external relays configured (evermore:relays.external) -- \
             running on the local control-plane relay only"
        );
    }
    for url in external_relays {
        let mention_store = Arc::clone(&store);
        let mention_filters = serde_json::json!({"kinds": [1], "#p": [own_pubkey]});
        relay_client::spawn_subscription(
            "mention",
            url.clone(),
            mention_filters,
            move |_label, event| handle_mention(event, &mention_store),
        );

        let dm_store = Arc::clone(&store);
        let dm_signer = signer.clone();
        let dm_filters = serde_json::json!({"kinds": [1059], "#p": [own_pubkey]});
        relay_client::spawn_subscription("dm", url, dm_filters, move |_label, event| {
            handle_gift_wrap(event, &dm_signer, &dm_store);
        });
    }

    if let Err(e) = serve(store) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn notify(appname_suffix: &str, summary: &str, body: &str) {
    if let Err(e) = Notification::new()
        .appname(&format!("Evermore{appname_suffix}"))
        .summary(summary)
        .body(body)
        .show()
    {
        eprintln!("evermore-notifyd: failed to show desktop notification: {e}");
    }
}

fn handle_local_event(event: Event, store: &NotificationStore) {
    let record = NotificationRecord {
        source: Source::Local,
        title: "System".to_string(),
        body: event.content.clone(),
        received_at: now(),
    };
    notify(" System", &record.title, &record.body);
    store.push(record);
}

fn handle_mention(event: Event, store: &NotificationStore) {
    let short_pubkey = short(&event.pubkey.to_hex());
    let record = NotificationRecord {
        source: Source::Mention,
        title: format!("Mention from {short_pubkey}"),
        body: event.content.clone(),
        received_at: now(),
    };
    notify("", &record.title, &record.body);
    store.push(record);
}

fn handle_gift_wrap(event: Event, signer: &SignerClient, store: &NotificationStore) {
    let rumor = match gift_wrap::unwrap(signer, &event) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("evermore-notifyd: couldn't unwrap a gift-wrapped event: {e}");
            return;
        }
    };
    let short_pubkey = short(&rumor.sender_pubkey);
    let record = NotificationRecord {
        source: Source::Dm,
        title: format!("DM from {short_pubkey} (kind {})", rumor.kind),
        body: rumor.content,
        received_at: now(),
    };
    notify(" DM", &record.title, &record.body);
    store.push(record);
}

fn short(pubkey_hex: &str) -> String {
    if pubkey_hex.len() > 12 {
        format!(
            "{}…{}",
            &pubkey_hex[..6],
            &pubkey_hex[pubkey_hex.len() - 6..]
        )
    } else {
        pubkey_hex.to_string()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn serve(store: Arc<NotificationStore>) -> std::io::Result<()> {
    let socket_path = evermore_common::notifications::runtime_socket_path();
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    eprintln!("evermore-notifyd listening on {}", socket_path.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let store = Arc::clone(&store);
                std::thread::spawn(move || {
                    if let Err(e) = handle_connection(stream, &store) {
                        eprintln!("connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
    Ok(())
}

fn handle_connection(stream: UnixStream, store: &NotificationStore) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(Request::ListRecent) => Response::ok(store.list_recent()),
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

fn parse_arg(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
