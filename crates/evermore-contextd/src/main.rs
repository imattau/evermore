use evermore_common::{ai, ai_context, notifications, settings};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    process::Command,
    sync::{Mutex, OnceLock},
};

static COMPOSITOR_CACHE: OnceLock<Mutex<Option<Value>>> = OnceLock::new();

fn main() {
    if let Err(error) = serve() {
        eprintln!("evermore-contextd: {error}");
        std::process::exit(1);
    }
}

fn serve() -> Result<(), String> {
    let path = ai_context::runtime_socket_path();
    let parent = path.parent().ok_or("context socket has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    eprintln!("evermore-contextd listening on {}", path.display());
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                std::thread::spawn(|| {
                    if let Err(e) = handle(stream) {
                        eprintln!("context connection: {e}");
                    }
                });
            }
            Err(e) => eprintln!("context accept: {e}"),
        }
    }
    Ok(())
}

fn handle(stream: UnixStream) -> Result<(), String> {
    let mut writer = BufWriter::new(stream.try_clone().map_err(|e| e.to_string())?);
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let result = serde_json::from_str::<ai_context::Request>(&line)
            .map_err(|e| format!("invalid context request: {e}"))
            .and_then(execute);
        let event = match result {
            Ok(e) => e,
            Err(message) => ai_context::Event::Error { message },
        };
        serde_json::to_writer(&mut writer, &event).map_err(|e| e.to_string())?;
        writer.write_all(b"\n").map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn execute(request: ai_context::Request) -> Result<ai_context::Event, String> {
    match request {
        ai_context::Request::Health => Ok(ai_context::Event::Health {
            protocol_version: ai_context::PROTOCOL_VERSION,
            ready: true,
        }),
        ai_context::Request::ListSources => Ok(ai_context::Event::Sources {
            sources: allowed_sources()?,
        }),
        ai_context::Request::Get { source } => {
            if !allowed_sources()?.contains(&source) {
                return Err("context source is not enabled in EVERMORE_CONTEXT_SOURCES".into());
            }
            let data = match source.as_str() {
                "compositor" => compositor_context()?,
                "settings:theme" => settings_theme()?,
                "notifications:local" => notification_context("local")?,
                "notifications:mention" => notification_context("mention")?,
                "packages:installed" => installed_packages()?,
                _ => return Err("unsupported context source".into()),
            };
            Ok(ai_context::Event::Context { source, data })
        }
        ai_context::Request::UpdateCompositor { data } => {
            let normalized = normalize_compositor(data)?;
            *COMPOSITOR_CACHE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .map_err(|e| e.to_string())? = Some(normalized);
            Ok(ai_context::Event::Updated {
                source: "compositor".into(),
            })
        }
    }
}

fn allowed_sources() -> Result<Vec<String>, String> {
    let raw = match std::env::var("EVERMORE_CONTEXT_SOURCES") {
        Ok(raw) => raw,
        Err(_) => {
            let path = ai_context::config_file_path();
            if path.is_file() {
                std::fs::read_to_string(path)
                    .map_err(|e| format!("reading context source selection: {e}"))?
            } else {
                "[]".into()
            }
        }
    };
    let mut sources: Vec<String> = serde_json::from_str(&raw)
        .map_err(|e| format!("context source selection must be a JSON array: {e}"))?;
    if let Some(unsupported) = sources.iter().find(|s| {
        ![
            "compositor",
            "settings:theme",
            "notifications:local",
            "notifications:mention",
            "packages:installed",
        ]
        .contains(&s.as_str())
    }) {
        return Err(format!("unsupported context source '{unsupported}'"));
    }
    sources.sort();
    sources.dedup();
    Ok(sources)
}

fn installed_packages() -> Result<Value, String> {
    let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/.local/share")
    });
    let root = std::path::PathBuf::from(base).join("evermore/npack");
    if !root.is_dir() {
        return Ok(json!({ "packages": [] }));
    }
    let canonical_root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut packages = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let package_dir = entry.path();
        let Ok(canonical_package_dir) = package_dir.canonicalize() else {
            continue;
        };
        let current = package_dir.join("current");
        let Ok(version_dir) = current.canonicalize() else {
            continue;
        };
        if !version_dir.starts_with(&canonical_root)
            || !version_dir.starts_with(&canonical_package_dir)
        {
            continue;
        }
        let manifest_path = version_dir.join("manifest.json");
        if !std::fs::symlink_metadata(&manifest_path).is_ok_and(|meta| meta.file_type().is_file()) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if manifest.get("id").and_then(Value::as_str) != entry.file_name().to_str() {
            continue;
        }
        packages.push(json!({
            "id": manifest.get("id").and_then(Value::as_str),
            "name": manifest.get("name").and_then(Value::as_str),
            "version": manifest.get("version").and_then(Value::as_str),
            "description": ai::redact_text(manifest.get("description").and_then(Value::as_str).unwrap_or(""))
        }));
    }
    packages.sort_by(|a, b| {
        a.get("id")
            .and_then(Value::as_str)
            .cmp(&b.get("id").and_then(Value::as_str))
    });
    Ok(json!({ "packages": packages }))
}

fn settings_theme() -> Result<Value, String> {
    match request_socket::<settings::Request, settings::Response>(
        settings::runtime_socket_path(),
        &settings::Request::GetDomain {
            domain: "theme".into(),
        },
    )? {
        settings::Response::Ok { data } => Ok(redact_value(data)),
        settings::Response::Err { message } => Err(message),
    }
}

fn notification_context(source: &str) -> Result<Value, String> {
    match request_socket::<notifications::Request, notifications::Response>(
        notifications::runtime_socket_path(),
        &notifications::Request::ListRecent,
    )? {
        notifications::Response::Ok { data } => {
            let records = data
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|record| record.get("source").and_then(Value::as_str) == Some(source))
                .map(redact_value)
                .collect::<Vec<_>>();
            Ok(json!({ "records": records }))
        }
        notifications::Response::Err { message } => Err(message),
    }
}

fn compositor_context() -> Result<Value, String> {
    if let Some(cached) = COMPOSITOR_CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
    {
        let observed_at = cached
            .get("observed_at")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.saturating_sub(observed_at) <= 5 {
            return Ok(cached);
        }
    }
    let output = Command::new("swaymsg")
        .args(["-t", "get_tree", "-r"])
        .output()
        .map_err(|e| format!("running swaymsg: {e}"))?;
    if !output.status.success() {
        return Err(format!("swaymsg failed with status {}", output.status));
    }
    let tree: Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("invalid sway tree: {e}"))?;
    let (focused, workspace) = find_focused(&tree, None);
    Ok(redact_value(
        json!({ "focused_window": focused, "workspace": workspace,
            "observed_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() }),
    ))
}

fn normalize_compositor(value: Value) -> Result<Value, String> {
    if !value.is_object() {
        return Err("compositor context must be an object".into());
    }
    let workspace = value.get("workspace").and_then(Value::as_str);
    let focused = value.get("focused_window").filter(|value| !value.is_null()).map(|window| {
        if !window.is_object() { return Err("focused_window must be an object or null".to_string()); }
        let field = |name: &str| window.get(name).and_then(Value::as_str).map(ai::redact_text);
        Ok(json!({"title": field("title"), "app_id": field("app_id"), "window_class": field("window_class")}))
    }).transpose()?;
    let observed_at = value
        .get("observed_at")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        });
    Ok(
        json!({ "focused_window": focused, "workspace": workspace.map(ai::redact_text), "observed_at": observed_at }),
    )
}

fn find_focused(node: &Value, parent_workspace: Option<String>) -> (Option<Value>, Option<String>) {
    let workspace = if node.get("type").and_then(Value::as_str) == Some("workspace")
        && node.get("focused").and_then(Value::as_bool) == Some(true)
    {
        node.get("name").and_then(Value::as_str).map(str::to_owned)
    } else {
        parent_workspace
    };
    if node.get("focused").and_then(Value::as_bool) == Some(true) {
        let is_window = node
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "con" || kind == "floating_con");
        if is_window {
            let focused = Some(
                json!({"title": node.get("name"), "app_id": node.get("app_id"), "window_class": node.get("window_properties").and_then(|p| p.get("class"))}),
            );
            return (focused, workspace);
        }
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                let found = find_focused(child, workspace.clone());
                if found.0.is_some() {
                    return found;
                }
                if found.1.is_some() {
                    return found;
                }
            }
        }
    }
    (None, workspace)
}

fn redact_value(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(ai::redact_text(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_value).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, redact_value(v))).collect())
        }
        other => other,
    }
}

fn request_socket<Q: Serialize, R: for<'de> serde::Deserialize<'de>>(
    path: std::path::PathBuf,
    request: &Q,
) -> Result<R, String> {
    let mut stream = UnixStream::connect(path).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut stream, request).map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}
