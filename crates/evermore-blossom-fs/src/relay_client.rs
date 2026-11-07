//! Single-thread-owns-the-websocket relay client, same shape as
//! evermore-settingsd's: tungstenite's sync API can't cleanly split into
//! independent reader/writer halves, so one thread polls between reading
//! inbound events and draining an outgoing publish queue.

use nostr::prelude::*;
use std::net::TcpStream;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

pub const KIND_FILE_INDEX: u16 = 30301;

#[derive(Clone)]
pub struct RelayClient {
    outgoing: Sender<String>,
}

impl RelayClient {
    /// Connects to `url`, subscribes to this identity's file-index events,
    /// and calls `on_event` (from the connection's own thread) for each one.
    pub fn connect(
        url: &str,
        own_pubkey_hex: String,
        on_event: impl Fn(Event) + Send + 'static,
    ) -> Result<Self, String> {
        let (ws, _response) =
            tungstenite::connect(url).map_err(|e| format!("couldn't connect to relay at {url}: {e}"))?;

        let (tx, rx) = mpsc::channel::<String>();

        let sub = serde_json::json!([
            "REQ",
            "evermore-blossom-fs",
            {"kinds": [KIND_FILE_INDEX as u32], "authors": [own_pubkey_hex]}
        ]);
        tx.send(sub.to_string())
            .map_err(|_| "relay connection thread died before subscribing".to_string())?;

        thread::spawn(move || run_loop(ws, rx, on_event));

        Ok(Self { outgoing: tx })
    }

    pub fn publish(&self, event: &Event) -> Result<(), String> {
        let msg = serde_json::json!(["EVENT", event]).to_string();
        self.outgoing
            .send(msg)
            .map_err(|_| "relay connection thread is gone".to_string())
    }
}

fn run_loop(mut ws: WebSocket<MaybeTlsStream<TcpStream>>, outgoing: mpsc::Receiver<String>, on_event: impl Fn(Event)) {
    if let MaybeTlsStream::Plain(tcp) = ws.get_ref() {
        let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
    }

    loop {
        match ws.read() {
            Ok(Message::Text(text)) => handle_message(&text, &on_event),
            Ok(Message::Close(_)) => {
                eprintln!("evermore-blossom-fs: relay closed the connection");
                break;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => {
                eprintln!("evermore-blossom-fs: relay connection error: {e}");
                break;
            }
        }

        while let Ok(msg) = outgoing.try_recv() {
            if let Err(e) = ws.send(Message::text(msg)) {
                eprintln!("evermore-blossom-fs: failed to send to relay: {e}");
            }
        }
    }
}

fn handle_message(text: &str, on_event: &impl Fn(Event)) {
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
