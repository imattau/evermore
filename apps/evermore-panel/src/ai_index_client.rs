use evermore_common::ai_index::{self, Event, Request, Scope, SearchResult};
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

pub fn list_scopes() -> Result<Vec<Scope>, String> {
    match request(&Request::ListScopes)? {
        Event::Scopes { scopes } => Ok(scopes),
        Event::Error { message } => Err(message),
        _ => Err("unexpected response from index service".into()),
    }
}

pub fn reindex(scope_id: &str) -> Result<(usize, usize), String> {
    match request(&Request::Reindex {
        scope_id: scope_id.into(),
    })? {
        Event::Indexed {
            documents, chunks, ..
        } => Ok((documents, chunks)),
        Event::Error { message } => Err(message),
        _ => Err("unexpected response from index service".into()),
    }
}

pub fn remove_scope(scope_id: &str) -> Result<(), String> {
    match request(&Request::RemoveScope {
        scope_id: scope_id.into(),
    })? {
        Event::Removed { .. } => Ok(()),
        Event::Error { message } => Err(message),
        _ => Err("unexpected response from index service".into()),
    }
}

pub fn search(query: &str) -> Result<Vec<SearchResult>, String> {
    match request(&Request::Search {
        query: query.into(),
        limit: Some(5),
    })? {
        Event::Results { results, .. } => Ok(results),
        Event::Error { message } => Err(message),
        _ => Err("unexpected response from index service".into()),
    }
}

pub fn add_file(path: &str) -> Result<(), String> {
    if std::env::var_os("EVERMORE_INDEX_FILES").is_some() {
        return Err("Index files are configured by EVERMORE_INDEX_FILES; edit that service setting and restart evermore-indexd.".into());
    }
    let input = std::path::PathBuf::from(path.trim());
    let metadata = std::fs::symlink_metadata(&input).map_err(|e| e.to_string())?;
    if !metadata.file_type().is_file() || metadata.len() > 2 * 1024 * 1024 {
        return Err("Choose a regular file no larger than 2 MiB.".into());
    }
    let extension = input
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["md", "txt", "rst", "org"].contains(&extension.as_str()) {
        return Err("Choose an .md, .txt, .rst, or .org file.".into());
    }
    let canonical = input
        .canonicalize()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let config = ai_index::config_file_path();
    let parent = config.parent().ok_or("index config path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut paths: Vec<String> = if config.exists() {
        serde_json::from_slice(&std::fs::read(&config).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid index selection file: {e}"))?
    } else {
        Vec::new()
    };
    if paths.contains(&canonical) {
        return Ok(());
    }
    paths.push(canonical);
    write_selection(&config, &paths)
}

pub fn discover_nostr_files() -> Result<Vec<String>, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    let root = std::path::PathBuf::from(home).join("Nostr");
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !metadata.file_type().is_file() || metadata.len() > 2 * 1024 * 1024 {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !["md", "txt", "rst", "org"].contains(&extension.as_str()) {
            continue;
        }
        if let Ok(canonical) = path.canonicalize() {
            paths.push(canonical.to_string_lossy().into_owned());
        }
    }
    paths.sort();
    Ok(paths)
}

pub fn remove_file(path: &str) -> Result<(), String> {
    if std::env::var_os("EVERMORE_INDEX_FILES").is_some() {
        return Err("Index files are configured by EVERMORE_INDEX_FILES; edit that service setting and restart evermore-indexd.".into());
    }
    let config = ai_index::config_file_path();
    if !config.exists() {
        return Ok(());
    }
    let mut paths: Vec<String> =
        serde_json::from_slice(&std::fs::read(&config).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid index selection file: {e}"))?;
    paths.retain(|selected| selected != path);
    write_selection(&config, &paths)
}

fn write_selection(config: &std::path::Path, paths: &[String]) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let tmp = config.with_extension(format!("json.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(paths).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, config).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

fn request<T: Serialize>(request: &T) -> Result<Event, String> {
    let mut stream = UnixStream::connect(ai_index::runtime_socket_path())
        .map_err(|e| format!("connecting to evermore-indexd: {e}"))?;
    serde_json::to_writer(&mut stream, request).map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}
