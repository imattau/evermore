use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender};

use evermore_common::ai::{self, Backend, Event, Message, Request};

#[derive(Debug)]
pub enum ChatUpdate {
    Token(String),
    Complete,
    Error(String),
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Preferences {
    pub model: Option<String>,
    pub backend: Option<Backend>,
}

pub fn load_preferences() -> Preferences {
    let path = preferences_path();
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save_preferences(preferences: &Preferences) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let path = preferences_path();
    let parent = path
        .parent()
        .ok_or_else(|| "AI preferences path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(preferences).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())?;
    std::fs::rename(temporary, path).map_err(|error| error.to_string())
}

fn preferences_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    base.join("evermore").join("panel-ai.json")
}

pub fn list_models() -> Result<Vec<ai::Model>, String> {
    let mut stream = UnixStream::connect(ai::runtime_infer_socket_path())
        .map_err(|error| format!("local inference service is unavailable: {error}"))?;
    let request = serde_json::to_string(&Request::ListModels).map_err(|error| error.to_string())?;
    writeln!(stream, "{request}").map_err(|error| error.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    let event: Event = serde_json::from_str(&line).map_err(|error| error.to_string())?;
    match event {
        Event::Models { models } => Ok(models),
        Event::Error { message, .. } => Err(message),
        _ => Err("inference service returned an unexpected model-list response".into()),
    }
}

pub fn start_chat(model: String, backend: Backend, messages: Vec<Message>) -> Receiver<ChatUpdate> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        if let Err(error) = chat_worker(&model, backend, messages, &sender) {
            let _ = sender.send(ChatUpdate::Error(error));
        }
    });
    receiver
}

fn chat_worker(
    model: &str,
    backend: Backend,
    messages: Vec<Message>,
    sender: &Sender<ChatUpdate>,
) -> Result<(), String> {
    let mut stream = UnixStream::connect(ai::runtime_infer_socket_path())
        .map_err(|error| format!("local inference service is unavailable: {error}"))?;
    let request = Request::Chat {
        request_id: format!("panel-{}", unique_suffix()),
        model: model.to_string(),
        messages,
        backend,
        max_tokens: Some(512),
        temperature: Some(0.7),
    };
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&request).map_err(|error| error.to_string())?
    )
    .map_err(|error| error.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?
            == 0
        {
            return Err("inference stream ended before completion".into());
        }
        match serde_json::from_str::<Event>(&line).map_err(|error| error.to_string())? {
            Event::Token { text, .. } => {
                sender
                    .send(ChatUpdate::Token(text))
                    .map_err(|error| error.to_string())?;
            }
            Event::Completed { .. } => {
                let _ = sender.send(ChatUpdate::Complete);
                return Ok(());
            }
            Event::Error { message, .. } => {
                let _ = sender.send(ChatUpdate::Error(message));
                return Ok(());
            }
            Event::Started { .. } => {}
            _ => return Err("inference stream returned an unexpected event".into()),
        }
    }
}

fn unique_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}
