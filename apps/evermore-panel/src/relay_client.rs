//! Minimal one-shot relay access for the Agents screen.
//!
//! Agent definitions and the membership list are owner-authored events on the
//! local control-plane relay. The panel signs through `evermore-signer` and
//! publishes here; it never talks to the relay from the agent runtime.

use std::time::Duration;

use nostr::prelude::*;
use tungstenite::Message;

pub const DEFAULT_RELAY_URL: &str = "ws://127.0.0.1:4869";
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

pub fn query(relay_url: &str, filter: serde_json::Value) -> Result<Vec<Event>, String> {
    let (mut ws, _response) = tungstenite::connect(relay_url)
        .map_err(|error| format!("couldn't connect to relay at {relay_url}: {error}"))?;
    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_ref() {
        let _ = tcp.set_read_timeout(Some(Duration::from_millis(300)));
    }

    let sub_id = "evermore-panel-agents";
    ws.send(Message::text(
        serde_json::json!(["REQ", sub_id, filter]).to_string(),
    ))
    .map_err(|error| format!("subscribing: {error}"))?;

    let mut events = Vec::new();
    let deadline = std::time::Instant::now() + QUERY_TIMEOUT;
    while std::time::Instant::now() < deadline {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                match value.get(0).and_then(|value| value.as_str()) {
                    Some("EVENT") => {
                        if let Some(event_value) = value.get(2)
                            && let Ok(event) = serde_json::from_value::<Event>(event_value.clone())
                        {
                            events.push(event);
                        }
                    }
                    Some("EOSE") => break,
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(format!("relay connection error: {error}")),
        }
    }
    let _ = ws.send(Message::text(
        serde_json::json!(["CLOSE", sub_id]).to_string(),
    ));
    Ok(events)
}

pub fn publish(relay_url: &str, event: &Event) -> Result<(), String> {
    let (mut ws, _response) = tungstenite::connect(relay_url)
        .map_err(|error| format!("couldn't connect to relay at {relay_url}: {error}"))?;
    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_ref() {
        let _ = tcp.set_read_timeout(Some(QUERY_TIMEOUT));
    }
    ws.send(Message::text(
        serde_json::json!(["EVENT", event]).to_string(),
    ))
    .map_err(|error| format!("publishing: {error}"))?;
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if value.get(0).and_then(|value| value.as_str()) == Some("OK") {
                    if value
                        .get(2)
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
                    {
                        return Ok(());
                    }
                    let reason = value
                        .get(3)
                        .and_then(|value| value.as_str())
                        .unwrap_or("rejected");
                    return Err(format!("relay rejected event: {reason}"));
                }
            }
            Ok(Message::Close(_)) => {
                return Err("relay closed the connection before OK".to_string());
            }
            Ok(_) => {}
            Err(error) => return Err(format!("publish connection error: {error}")),
        }
    }
}
