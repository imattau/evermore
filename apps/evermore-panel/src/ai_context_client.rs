use evermore_common::ai_context::{self, Event, Request};
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub const SOURCES: &[(&str, &str)] = &[
    ("compositor", "Focused window and workspace"),
    ("settings:theme", "Theme settings"),
    ("notifications:local", "Local notification metadata"),
    ("notifications:mention", "Mention notification metadata"),
    ("packages:installed", "Installed package metadata"),
    ("calendar:events", "Nostr calendar event details"),
];

pub fn list_sources() -> Result<Vec<String>, String> {
    match request(&Request::ListSources)? {
        Event::Sources { sources } => Ok(sources),
        Event::Error { message } => Err(message),
        _ => Err("unexpected response from context service".into()),
    }
}

pub fn get(source: &str) -> Result<serde_json::Value, String> {
    let payload = if let Some(path) = source.strip_prefix("file:") {
        Request::GetIndexedFile { path: path.into() }
    } else {
        Request::Get {
            source: source.into(),
        }
    };
    match request(&payload)? {
        Event::Context { data, .. } => Ok(data),
        Event::Error { message } => Err(message),
        _ => Err("unexpected response from context service".into()),
    }
}

pub fn save_sources(sources: &[String]) -> Result<(), String> {
    if std::env::var_os("EVERMORE_CONTEXT_SOURCES").is_some() {
        return Err("Context sources are configured by EVERMORE_CONTEXT_SOURCES; edit that service setting and restart evermore-contextd.".into());
    }
    if let Some(source) = sources
        .iter()
        .find(|source| !SOURCES.iter().any(|(allowed, _)| allowed == source))
    {
        return Err(format!("Unsupported context source: {source}"));
    }
    let mut sources = sources.to_vec();
    sources.sort();
    sources.dedup();
    let config = ai_context::config_file_path();
    let parent = config.parent().ok_or("context config path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = config.with_extension(format!("json.{}.tmp", std::process::id()));
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, &sources).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&temp, &config).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        e.to_string()
    })
}

fn request<T: Serialize>(request: &T) -> Result<Event, String> {
    let mut stream = UnixStream::connect(ai_context::runtime_socket_path())
        .map_err(|e| format!("connecting to evermore-contextd: {e}"))?;
    serde_json::to_writer(&mut stream, request).map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}
