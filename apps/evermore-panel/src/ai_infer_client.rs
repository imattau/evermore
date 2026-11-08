use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use evermore_common::ai::{self, Backend, Event, Message, Request};

#[derive(Debug)]
pub enum ChatUpdate {
    Token(String),
    Complete,
    Cancelled,
    Error(String),
}

pub struct ChatTask {
    pub receiver: Receiver<ChatUpdate>,
    pub cancellation: ChatCancellation,
}

#[derive(Clone)]
pub struct ChatCancellation(Arc<ChatControl>);

struct ChatControl {
    cancelled: AtomicBool,
    stream: Mutex<Option<UnixStream>>,
}

impl ChatCancellation {
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        if let Ok(stream) = self.0.stream.lock()
            && let Some(stream) = stream.as_ref()
        {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Drop for ChatCancellation {
    fn drop(&mut self) {
        if Arc::strong_count(&self.0) == 1 {
            self.cancel();
        }
    }
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
    list_models_with_timeout(None)
}

pub fn list_models_with_timeout(
    timeout: Option<std::time::Duration>,
) -> Result<Vec<ai::Model>, String> {
    list_models_at_path(ai::runtime_infer_socket_path(), timeout)
}

fn list_models_at_path(
    socket_path: std::path::PathBuf,
    timeout: Option<std::time::Duration>,
) -> Result<Vec<ai::Model>, String> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|error| format!("local inference service is unavailable: {error}"))?;
    if let Some(timeout) = timeout {
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|error| format!("setting model-list deadline: {error}"))?;
    }
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

#[cfg(test)]
mod tests {
    use super::{ChatCancellation, ChatControl, list_models_at_path};
    use std::io::Read;
    use std::os::unix::net::UnixListener;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn timed_model_discovery_returns_when_daemon_stalls() {
        let socket_path = std::env::temp_dir().join(format!(
            "evermore-infer-timeout-{}-{}.sock",
            std::process::id(),
            super::unique_suffix()
        ));
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = std::thread::spawn(move || {
            let (_connection, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });

        let started = Instant::now();
        let result = list_models_at_path(socket_path.clone(), Some(Duration::from_millis(30)));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(150));
        let _ = std::fs::remove_file(socket_path);
        server.join().unwrap();
    }

    #[test]
    fn cancelling_chat_shuts_down_its_inference_socket() {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let cancellation = ChatCancellation(Arc::new(ChatControl {
            cancelled: AtomicBool::new(false),
            stream: Mutex::new(Some(stream)),
        }));

        cancellation.cancel();

        let mut byte = [0_u8; 1];
        assert_eq!(peer.read(&mut byte).unwrap(), 0);
        assert!(
            cancellation
                .0
                .cancelled
                .load(std::sync::atomic::Ordering::SeqCst)
        );
    }
}

pub fn start_chat(model: String, backend: Backend, messages: Vec<Message>) -> ChatTask {
    start_chat_with_policy(model, backend, messages, false)
}

pub fn start_chat_if_warm(model: String, backend: Backend, messages: Vec<Message>) -> ChatTask {
    start_chat_with_policy(model, backend, messages, true)
}

fn start_chat_with_policy(
    model: String,
    backend: Backend,
    messages: Vec<Message>,
    warm_only: bool,
) -> ChatTask {
    let (sender, receiver) = mpsc::channel();
    let control = Arc::new(ChatControl {
        cancelled: AtomicBool::new(false),
        stream: Mutex::new(None),
    });
    let worker_control = control.clone();
    std::thread::spawn(move || {
        if let Err(error) = chat_worker(
            &model,
            backend,
            messages,
            &sender,
            &worker_control,
            warm_only,
        ) && !worker_control.cancelled.load(Ordering::SeqCst)
        {
            let _ = sender.send(ChatUpdate::Error(error));
        }
    });
    ChatTask {
        receiver,
        cancellation: ChatCancellation(control),
    }
}

fn chat_worker(
    model: &str,
    backend: Backend,
    messages: Vec<Message>,
    sender: &Sender<ChatUpdate>,
    control: &Arc<ChatControl>,
    warm_only: bool,
) -> Result<(), String> {
    let operation_proposal = messages.iter().any(|message| {
        message.role == evermore_common::ai::Role::System
            && message.content.contains("EVERMORE_PROPOSAL:")
    });
    let mut stream = UnixStream::connect(ai::runtime_infer_socket_path())
        .map_err(|error| format!("local inference service is unavailable: {error}"))?;
    let cancel_stream = stream.try_clone().map_err(|error| error.to_string())?;
    *control.stream.lock().map_err(|error| error.to_string())? = Some(cancel_stream);
    if control.cancelled.load(Ordering::SeqCst) {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        let _ = sender.send(ChatUpdate::Cancelled);
        return Ok(());
    }
    let request = Request::Chat {
        request_id: format!("panel-{}", unique_suffix()),
        model: model.to_string(),
        messages,
        backend,
        max_tokens: Some(if operation_proposal { 160 } else { 512 }),
        temperature: Some(if operation_proposal { 0.1 } else { 0.7 }),
        grammar: None,
        warm_only,
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
            if control.cancelled.load(Ordering::SeqCst) {
                let _ = sender.send(ChatUpdate::Cancelled);
                return Ok(());
            }
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
