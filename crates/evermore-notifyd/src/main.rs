mod gift_wrap;
mod notification_store;
mod relay_client;
mod settings_client;
mod signer_client;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

    spawn_approval_monitor();

    if let Err(e) = serve(store) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn notify(appname: &str, summary: &str, body: &str) {
    if let Err(e) = Notification::new()
        .appname(appname)
        .summary(summary)
        .body(body)
        .show()
    {
        eprintln!("evermore-notifyd: failed to show desktop notification: {e}");
    }
}

fn handle_local_event(event: Event, store: &NotificationStore) {
    let record = NotificationRecord {
        id: String::new(),
        source: Source::Local,
        title: "System".to_string(),
        body: event.content.clone(),
        received_at: now(),
    };
    notify("Evermore · System", &record.title, &record.body);
    store.push(record);
}

fn handle_mention(event: Event, store: &NotificationStore) {
    let short_pubkey = short(&event.pubkey.to_hex());
    let record = NotificationRecord {
        id: String::new(),
        source: Source::Mention,
        title: format!("Mention from {short_pubkey}"),
        body: event.content.clone(),
        received_at: now(),
    };
    notify(
        &format!("Evermore · Mentions · {short_pubkey}"),
        &record.title,
        &record.body,
    );
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
        id: String::new(),
        source: Source::Dm,
        title: format!("DM from {short_pubkey} (kind {})", rumor.kind),
        body: rumor.content,
        received_at: now(),
    };
    notify(
        &format!("Evermore · Messages · {short_pubkey}"),
        &record.title,
        &record.body,
    );
    store.push(record);
}

/// Surface pending owner approvals through the same desktop notification
/// daemon. The toast is only a deep link: approval still requires the normal
/// signer identity, grant check, and explicit decision in Evermore.
fn spawn_approval_monitor() {
    std::thread::spawn(|| {
        let seen = Mutex::new(std::collections::HashSet::<String>::new());
        loop {
            match pending_operations() {
                Ok(pending) => {
                    let active: std::collections::HashSet<String> = pending
                        .iter()
                        .filter_map(|item| {
                            item.get("operation_id")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect();
                    if let Ok(mut known) = seen.lock() {
                        known.retain(|id| active.contains(id));
                        for item in pending {
                            let Some(operation_id) =
                                item.get("operation_id").and_then(serde_json::Value::as_str)
                            else {
                                continue;
                            };
                            if !known.insert(operation_id.to_owned()) {
                                continue;
                            }
                            let operation = item
                                .get("operation")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("operation")
                                .to_owned();
                            let created_at = item
                                .get("created_at")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or_default();
                            let expires_at = item
                                .get("expires_at")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or_default();
                            if expires_at > 0 && expires_at <= now() {
                                continue;
                            }
                            show_approval_notification(
                                operation_id.to_owned(),
                                operation,
                                created_at,
                                expires_at,
                            );
                        }
                    }
                }
                Err(error) => eprintln!("evermore-notifyd: approval monitor: {error}"),
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

fn pending_operations() -> Result<Vec<serde_json::Value>, String> {
    use evermore_common::operations::{Request, Response};
    let path = evermore_common::operations::runtime_socket_path();
    let mut stream = UnixStream::connect(&path)
        .map_err(|error| format!("connecting to operations daemon: {error}"))?;
    let request = serde_json::to_string(&Request::ListPendingOperations)
        .map_err(|error| error.to_string())?;
    writeln!(stream, "{request}").map_err(|error| error.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    let data = match serde_json::from_str::<Response>(&line).map_err(|e| e.to_string())? {
        Response::Ok { data } => data,
        Response::Err { message } => return Err(message),
    };
    serde_json::from_value(data).map_err(|error| error.to_string())
}

fn show_approval_notification(
    operation_id: String,
    operation: String,
    created_at: u64,
    expires_at: u64,
) {
    let mut body = format!("Owner review required · request {}", short(&operation_id));
    if created_at > 0 && expires_at > 0 {
        body.push_str(&format!(" · expires {expires_at}"));
    }
    match Notification::new()
        .appname(&format!("Evermore · Approval · {operation}"))
        .summary("Operation needs approval")
        .body(&body)
        .urgency(notify_rust::Urgency::Critical)
        .action("review", "Review")
        .show()
    {
        Ok(handle) => {
            std::thread::spawn(move || {
                handle.wait_for_action(move |action| {
                    if action == "review" || action == "default" {
                        if let Err(error) = std::process::Command::new("evermore-panel")
                            .arg("--approval")
                            .arg(operation_id)
                            .spawn()
                        {
                            eprintln!("evermore-notifyd: couldn't open approval review: {error}");
                        }
                    }
                });
            });
        }
        Err(error) => eprintln!("evermore-notifyd: failed to show approval notification: {error}"),
    }
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
            Ok(Request::ClearRecent) => {
                Response::ok(serde_json::json!({"cleared": store.clear_recent()}))
            }
            Ok(Request::Dismiss { notification_id }) => match store.dismiss(&notification_id) {
                Ok(dismissed) => Response::ok(serde_json::json!({"dismissed": dismissed})),
                Err(message) => Response::err(message),
            },
            Ok(Request::SetDnd { enabled }) => match set_dnd(enabled) {
                Ok(data) => Response::ok(data),
                Err(message) => Response::err(message),
            },
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

fn set_dnd(enabled: bool) -> Result<serde_json::Value, String> {
    let output = std::process::Command::new("swaync-client")
        .args([dnd_flag(enabled), "-sw"])
        .output()
        .map_err(|error| format!("couldn't run swaync-client: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(if stderr.is_empty() {
            format!("swaync-client exited with {}", output.status)
        } else {
            format!("swaync-client failed: {stderr}")
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let dnd = parse_dnd_state(&stdout)?;
    if dnd != enabled {
        return Err(format!(
            "swaync-client reported DND {dnd} after requesting {enabled}"
        ));
    }
    Ok(serde_json::json!({ "dnd": dnd }))
}

fn dnd_flag(enabled: bool) -> &'static str {
    if enabled { "-dn" } else { "-df" }
}

fn parse_dnd_state(stdout: &str) -> Result<bool, String> {
    let state = stdout.trim().to_ascii_lowercase();
    if state.ends_with("true") || state == "on" {
        Ok(true)
    } else if state.ends_with("false") || state == "off" {
        Ok(false)
    } else {
        Err(format!(
            "swaync-client returned an unexpected DND state: {}",
            stdout.trim()
        ))
    }
}

fn parse_arg(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dnd_flags_select_the_requested_state() {
        assert_eq!(dnd_flag(true), "-dn");
        assert_eq!(dnd_flag(false), "-df");
    }

    #[test]
    fn dnd_state_output_is_parsed_conservatively() {
        assert!(parse_dnd_state("true\n").unwrap());
        assert!(!parse_dnd_state("DND: false").unwrap());
        assert!(parse_dnd_state("true").unwrap());
        assert!(!parse_dnd_state("off").unwrap());
        assert!(parse_dnd_state("maybe").is_err());
    }
}
