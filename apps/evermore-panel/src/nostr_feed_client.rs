//! Minimal Nostr relay client for the feed drawer: subscribes to recent
//! kind:1 notes on the user's configured external relays
//! (`evermore:relays.external`, see `settings_client::external_relay_urls`),
//! and can publish a signed reply back out to those same relays. Modeled on
//! evermore-settingsd's relay_client (subscribe plus an outgoing-publish
//! queue, one thread per relay) since that's the only other relay client in
//! this codebase that both reads and writes events.

use nostr::prelude::*;
use serde::{Deserialize, Serialize};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub pubkey: String,
    pub content: String,
    pub created_at: u64,
}

/// One outgoing queue per connected relay. Publishing a reply writes to all
/// of them, since the original note's thread may be read from any of them.
#[derive(Default)]
pub struct FeedHandle {
    senders: Vec<Sender<String>>,
}

impl FeedHandle {
    pub fn publish(&self, event: &serde_json::Value) {
        let message = serde_json::json!(["EVENT", event]).to_string();
        for sender in &self.senders {
            let _ = sender.send(message.clone());
        }
    }
}

/// Connects to every relay in `relay_urls`, subscribes each to the most
/// recent kind:1 notes, and forwards parsed notes to the returned receiver
/// as they arrive. The same note commonly arrives from more than one relay;
/// deduplicating by `id` is the caller's job. Connecting happens on the
/// per-relay thread so an unreachable relay cannot stall the drawer.
pub fn start(relay_urls: Vec<String>) -> (Receiver<Note>, FeedHandle) {
    let (notes_tx, notes_rx) = mpsc::channel();
    let mut senders = Vec::new();
    for url in relay_urls {
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<String>();
        senders.push(outgoing_tx);
        let notes_tx = notes_tx.clone();
        thread::spawn(move || {
            let mut ws = match tungstenite::connect(&url) {
                Ok((connection, _response)) => connection,
                Err(error) => {
                    eprintln!("evermore-panel: couldn't connect to feed relay at {url}: {error}");
                    return;
                }
            };
            let subscribe =
                serde_json::json!(["REQ", "evermore-panel-feed", {"kinds": [1], "limit": 50}]);
            if ws.send(Message::text(subscribe.to_string())).is_err() {
                return;
            }
            run_loop(ws, outgoing_rx, notes_tx);
        });
    }
    (notes_rx, FeedHandle { senders })
}

fn run_loop(
    mut ws: WebSocket<MaybeTlsStream<TcpStream>>,
    outgoing: mpsc::Receiver<String>,
    notes_tx: Sender<Note>,
) {
    set_read_timeout(ws.get_ref(), Duration::from_millis(200));

    loop {
        match ws.read() {
            Ok(Message::Text(text)) => handle_message(&text, &notes_tx),
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
        while let Ok(message) = outgoing.try_recv() {
            if ws.send(Message::text(message)).is_err() {
                return;
            }
        }
    }
}

fn set_read_timeout(stream: &MaybeTlsStream<TcpStream>, timeout: Duration) {
    match stream {
        MaybeTlsStream::Plain(tcp) => {
            let _ = tcp.set_read_timeout(Some(timeout));
        }
        MaybeTlsStream::Rustls(tls) => {
            let _ = tls.get_ref().set_read_timeout(Some(timeout));
        }
        #[allow(unreachable_patterns)]
        _ => {}
    }
}

fn handle_message(text: &str, notes_tx: &Sender<Note>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    if value.get(0).and_then(|v| v.as_str()) != Some("EVENT") {
        return;
    }
    let Some(event_value) = value.get(2) else {
        return;
    };
    let Ok(event) = serde_json::from_value::<Event>(event_value.clone()) else {
        return;
    };
    let _ = notes_tx.send(Note {
        id: event.id.to_hex(),
        pubkey: event.pubkey.to_hex(),
        content: event.content,
        created_at: event.created_at.as_secs(),
    });
}
