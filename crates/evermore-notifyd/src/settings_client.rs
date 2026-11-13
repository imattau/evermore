//! Reads the "relays" settings domain to find external relays to also
//! subscribe to (`evermore:relays` -> key `external`, a JSON array of
//! relay URLs). Entirely optional: if evermore-settingsd isn't running or
//! the key isn't set, notifyd just runs on the local control-plane relay.

use evermore_common::settings::{Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub fn external_relay_urls() -> Vec<String> {
    match get_domain("relays") {
        Ok(map) => map
            .get("external")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Returns whether a notification source is enabled. Missing settings or an
/// unavailable settings service preserve the previous default-on behavior.
pub fn notification_source_enabled(key: &str) -> bool {
    get_domain("notifications")
        .ok()
        .and_then(|map| map.get(key).cloned())
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}

/// Returns the identity's quiet-hours schedule when one is enabled.
pub fn notification_schedule() -> Result<Option<(String, String)>, String> {
    let map = get_domain("notifications")?;
    if !map
        .get("schedule_enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let start = map
        .get("quiet_start")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "quiet-hours start time is missing".to_string())?;
    let end = map
        .get("quiet_end")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "quiet-hours end time is missing".to_string())?;
    Ok(Some((start.to_owned(), end.to_owned())))
}

fn get_domain(domain: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let path = evermore_common::settings::runtime_socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|e| e.to_string())?;
    let mut line = serde_json::to_string(&Request::GetDomain {
        domain: domain.to_string(),
    })
    .map_err(|e| e.to_string())?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .map_err(|e| e.to_string())?;
    match serde_json::from_str::<Response>(&response_line).map_err(|e| e.to_string())? {
        Response::Ok { data } => serde_json::from_value(data).map_err(|e| e.to_string()),
        Response::Err { message } => Err(message),
    }
}
