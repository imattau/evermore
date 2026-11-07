use evermore_common::ai_context::{self, Event, Request};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    process::Command,
    time::Duration,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("evermore-sway-ai-bridge: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut previous: Option<Value> = None;
    loop {
        match current_compositor_context() {
            Ok(context) if previous.as_ref() != Some(&context) => {
                match send(&Request::UpdateCompositor {
                    data: context.clone(),
                }) {
                    Ok(Event::Updated { .. }) => previous = Some(context),
                    Ok(Event::Error { message }) => {
                        eprintln!("context daemon rejected update: {message}")
                    }
                    Ok(_) => eprintln!("context daemon returned an unexpected response"),
                    Err(error) => eprintln!("context daemon unavailable: {error}"),
                }
            }
            Ok(_) => {}
            Err(error) => eprintln!("could not read Sway state: {error}"),
        }
        std::thread::sleep(Duration::from_millis(750));
    }
}

fn current_compositor_context() -> Result<Value, String> {
    let output = Command::new("swaymsg")
        .args(["-t", "get_tree", "-r"])
        .output()
        .map_err(|e| format!("running swaymsg: {e}"))?;
    if !output.status.success() {
        return Err(format!("swaymsg exited with {}", output.status));
    }
    let tree: Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("invalid Sway tree: {e}"))?;
    let (focused, workspace) = find_focused(&tree, None);
    let observed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(json!({ "focused_window": focused, "workspace": workspace, "observed_at": observed_at }))
}

fn find_focused(node: &Value, parent_workspace: Option<String>) -> (Option<Value>, Option<String>) {
    let workspace = if node.get("type").and_then(Value::as_str) == Some("workspace")
        && node.get("focused").and_then(Value::as_bool) == Some(true)
    {
        node.get("name").and_then(Value::as_str).map(str::to_owned)
    } else {
        parent_workspace
    };
    if node.get("focused").and_then(Value::as_bool) == Some(true)
        && node
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "con" || kind == "floating_con")
    {
        return (
            Some(json!({
                "title": node.get("name").and_then(Value::as_str),
                "app_id": node.get("app_id").and_then(Value::as_str),
                "window_class": node.get("window_properties").and_then(|p| p.get("class")).and_then(Value::as_str)
            })),
            workspace,
        );
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                let found = find_focused(child, workspace.clone());
                if found.0.is_some() || found.1.is_some() {
                    return found;
                }
            }
        }
    }
    (None, workspace)
}

fn send<T: Serialize>(request: &T) -> Result<Event, String> {
    let mut stream = UnixStream::connect(ai_context::runtime_socket_path())
        .map_err(|e| format!("connecting to ai-contextd: {e}"))?;
    serde_json::to_writer(&mut stream, request).map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| e.to_string())
}
