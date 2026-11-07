//! Thin blocking client for evermore-settingsd's Unix socket, same pattern
//! as `signer_client`.

use evermore_common::settings::{Request, Response};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub fn get_domain(domain: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match request(Request::GetDomain {
        domain: domain.to_string(),
    })? {
        Response::Ok { data } => serde_json::from_value(data).map_err(|e| e.to_string()),
        Response::Err { message } => Err(message),
    }
}

pub fn set(domain: &str, key: &str, value: serde_json::Value) -> Result<(), String> {
    match request(Request::Set {
        domain: domain.to_string(),
        key: key.to_string(),
        value,
    })? {
        Response::Ok { .. } => Ok(()),
        Response::Err { message } => Err(message),
    }
}

fn request(req: Request) -> Result<Response, String> {
    let path = evermore_common::settings::runtime_socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|e| {
        format!(
            "couldn't connect to evermore-settingsd at {} ({e}) -- is it running? (`evermore-settingsd --identity NAME`)",
            path.display()
        )
    })?;
    let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
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
    serde_json::from_str(&response_line).map_err(|e| e.to_string())
}
