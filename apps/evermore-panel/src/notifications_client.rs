//! Thin blocking client for evermore-notifyd's Unix socket.

use evermore_common::notifications::{NotificationRecord, Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub fn list_recent() -> Result<Vec<NotificationRecord>, String> {
    let path = evermore_common::notifications::runtime_socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|e| {
        format!(
            "couldn't connect to evermore-notifyd at {} ({e}) -- is it running? (`evermore-notifyd --identity NAME`)",
            path.display()
        )
    })?;
    let mut line = serde_json::to_string(&Request::ListRecent).map_err(|e| e.to_string())?;
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
