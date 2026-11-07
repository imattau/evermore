//! One-shot relay access: npack is a plain CLI, not a daemon, so unlike
//! the other components' relay_client.rs (which own a persistent
//! connection on a dedicated thread) this just opens a connection, does
//! one thing, and closes it.

use nostr::prelude::*;
use std::time::Duration;
use tungstenite::Message;

const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Sends a `REQ` with `filter`, collects events until `EOSE` or the
/// timeout, then closes the connection.
pub fn query(relay_url: &str, filter: serde_json::Value) -> Result<Vec<Event>, String> {
    let (mut ws, _response) = tungstenite::connect(relay_url)
        .map_err(|e| format!("couldn't connect to relay at {relay_url}: {e}"))?;

    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_ref() {
        let _ = tcp.set_read_timeout(Some(Duration::from_millis(300)));
    }

    let sub_id = "evermore-npack";
    let req = serde_json::json!(["REQ", sub_id, filter]);
    ws.send(Message::text(req.to_string()))
        .map_err(|e| format!("subscribing: {e}"))?;

    let mut events = Vec::new();
    let deadline = std::time::Instant::now() + QUERY_TIMEOUT;
    while std::time::Instant::now() < deadline {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                match value.get(0).and_then(|v| v.as_str()) {
                    Some("EVENT") => {
                        if let Some(evt_val) = value.get(2)
                            && let Ok(event) = serde_json::from_value::<Event>(evt_val.clone())
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
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(format!("relay connection error: {e}")),
        }
    }
    let _ = ws.send(Message::text(
        serde_json::json!(["CLOSE", sub_id]).to_string(),
    ));
    Ok(events)
}

/// Publishes `event` and waits for the relay's `OK` response.
pub fn publish(relay_url: &str, event: &Event) -> Result<(), String> {
    let (mut ws, _response) = tungstenite::connect(relay_url)
        .map_err(|e| format!("couldn't connect to relay at {relay_url}: {e}"))?;
    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_ref() {
        let _ = tcp.set_read_timeout(Some(QUERY_TIMEOUT));
    }

    let msg = serde_json::json!(["EVENT", event]);
    ws.send(Message::text(msg.to_string()))
        .map_err(|e| format!("publishing: {e}"))?;

    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if value.get(0).and_then(|v| v.as_str()) == Some("OK") {
                    let accepted = value.get(2).and_then(|v| v.as_bool()).unwrap_or(false);
                    if accepted {
                        return Ok(());
                    }
                    let reason = value.get(3).and_then(|v| v.as_str()).unwrap_or("rejected");
                    return Err(format!("relay rejected event: {reason}"));
                }
            }
            Ok(Message::Close(_)) => {
                return Err("relay closed the connection before OK".to_string());
            }
            Ok(_) => {}
            Err(e) => return Err(format!("publish connection error: {e}")),
        }
    }
}
