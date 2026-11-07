//! Read-only relay subscription. notifyd never publishes, so unlike
//! evermore-settingsd's relay_client this doesn't need an outgoing queue --
//! just a thread per relay blocking on reads and forwarding EVENT messages.

use nostr::prelude::*;
use std::thread;

/// Connects to `url`, sends a single `REQ` with `filters_json` (a
/// pre-built NIP-01 filter object), and calls `on_event` for each matching
/// event for as long as the connection lives. Runs forever on its own
/// thread; logs and returns (without retrying) on a fatal error -- good
/// enough for a first version, reconnect-with-backoff is a follow-up.
pub fn spawn_subscription(
    label: &'static str,
    url: String,
    filters_json: serde_json::Value,
    on_event: impl Fn(&'static str, Event) + Send + 'static,
) {
    thread::spawn(move || {
        let (mut ws, _resp) = match tungstenite::connect(&url) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("evermore-notifyd: couldn't connect to {label} relay at {url}: {e}");
                return;
            }
        };

        let req = serde_json::json!(["REQ", format!("evermore-notifyd-{label}"), filters_json]);
        if let Err(e) = ws.send(tungstenite::Message::text(req.to_string())) {
            eprintln!("evermore-notifyd: couldn't subscribe on {label} relay: {e}");
            return;
        }
        eprintln!("evermore-notifyd: subscribed to {label} relay at {url}");

        loop {
            match ws.read() {
                Ok(tungstenite::Message::Text(text)) => handle_message(label, &text, &on_event),
                Ok(tungstenite::Message::Close(_)) => {
                    eprintln!("evermore-notifyd: {label} relay closed the connection");
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    eprintln!("evermore-notifyd: {label} relay connection error: {e}");
                    break;
                }
            }
        }
    });
}

fn handle_message(label: &'static str, text: &str, on_event: &impl Fn(&'static str, Event)) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    if value.get(0).and_then(|v| v.as_str()) != Some("EVENT") {
        return;
    }
    let Some(evt_val) = value.get(2) else {
        return;
    };
    if let Ok(event) = serde_json::from_value::<Event>(evt_val.clone()) {
        on_event(label, event);
    }
}
