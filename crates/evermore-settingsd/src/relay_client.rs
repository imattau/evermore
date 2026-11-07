//! A small synchronous client for the local control-plane relay. Owns the
//! websocket on a single dedicated thread (tungstenite's sync `WebSocket`
//! doesn't support splitting into independent reader/writer halves cleanly,
//! so rather than fight that, one thread polls for both inbound events and
//! outbound publish jobs) and reports NIP-01 `EVENT` messages for our own
//! subscription back to the caller via a callback.

use nostr::prelude::*;
use std::net::TcpStream;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

#[derive(Clone)]
pub struct RelayClient {
    outgoing: Sender<String>,
}

impl RelayClient {
    /// Connects to `url`, subscribes to kind:30078 events authored by
    /// `own_pubkey_hex`, and calls `on_event` (from the connection's own
    /// thread -- keep it quick/non-blocking) for each one received.
    pub fn connect(
        url: &str,
        own_pubkey_hex: String,
        on_event: impl Fn(Event) + Send + 'static,
    ) -> Result<Self, String> {
        let (ws, _response) = tungstenite::connect(url)
            .map_err(|e| format!("couldn't connect to relay at {url}: {e}"))?;

        let (tx, rx) = mpsc::channel::<String>();

        // Initial subscription, queued before the loop starts so it's the
        // first thing sent once the connection is live.
        let sub_id = "evermore-settingsd";
        let req = serde_json::json!([
            "REQ",
            sub_id,
            {"kinds": [crate::settings_store::KIND_SETTINGS], "authors": [own_pubkey_hex]}
        ]);
        tx.send(req.to_string())
            .map_err(|_| "relay connection thread died before subscribing".to_string())?;

        thread::spawn(move || run_loop(ws, rx, on_event));

        Ok(Self { outgoing: tx })
    }

    /// Queues a signed event for publishing (`["EVENT", {...}]`). Fire and
    /// forget: we don't wait for the relay's `OK` response here -- this is
    /// our own local, fully-controlled relay, not a flaky public one.
    pub fn publish(&self, event: &Event) -> Result<(), String> {
        let msg = serde_json::json!(["EVENT", event]).to_string();
        self.outgoing
            .send(msg)
            .map_err(|_| "relay connection thread is gone".to_string())
    }
}

fn run_loop(
    mut ws: WebSocket<MaybeTlsStream<TcpStream>>,
    outgoing: mpsc::Receiver<String>,
    on_event: impl Fn(Event),
) {
    if let MaybeTlsStream::Plain(tcp) = ws.get_ref() {
        let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
    }

    loop {
        match ws.read() {
            Ok(Message::Text(text)) => handle_relay_message(&text, &on_event),
            Ok(Message::Close(_)) => {
                eprintln!("relay closed the connection");
                break;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => {
                eprintln!("relay connection error: {e}");
                break;
            }
        }

        while let Ok(msg) = outgoing.try_recv() {
            if let Err(e) = ws.send(Message::text(msg)) {
                eprintln!("failed to send to relay: {e}");
            }
        }
    }
}

fn handle_relay_message(text: &str, on_event: &impl Fn(Event)) {
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
        on_event(event);
    }
}
