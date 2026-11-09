use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use evermore_common::ai::{self, Backend, Event, Message, Model, Request};

pub struct ChatResult {
    pub text: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

/// Pick a local chat model. Prefer an already-warm model with an adapter, then
/// any warm chat model, then any installed chat model.
pub fn select_chat_model() -> Result<String, String> {
    let models = list_models()?;
    let mut candidates: Vec<&Model> = models
        .iter()
        .filter(|model| {
            model.capabilities.is_empty()
                || model
                    .capabilities
                    .iter()
                    .any(|capability| capability == "chat")
        })
        .collect();
    candidates.sort_by_key(|model| {
        let explicit_chat = model
            .capabilities
            .iter()
            .any(|capability| capability == "chat");
        (
            !explicit_chat,
            !model.loaded,
            model.adapter.is_none(),
            model.name.clone(),
        )
    });
    candidates
        .first()
        .map(|model| model.name.clone())
        .ok_or_else(|| {
            "no local chat model is installed; install a signed model package first".to_string()
        })
}

fn list_models() -> Result<Vec<Model>, String> {
    let mut stream = connect()?;
    let request = serde_json::to_string(&Request::ListModels).map_err(|error| error.to_string())?;
    writeln!(stream, "{request}").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    match serde_json::from_str::<Event>(&line).map_err(|error| error.to_string())? {
        Event::Models { models } => Ok(models),
        Event::Error { message, .. } => Err(message),
        _ => Err("inference service returned an unexpected model-list response".into()),
    }
}

pub fn chat(
    model: &str,
    messages: Vec<Message>,
    grammar: Option<String>,
    max_tokens: u32,
) -> Result<ChatResult, String> {
    let mut stream = connect()?;
    let request = Request::Chat {
        request_id: format!("agentd-{}", unix_nanos()),
        model: model.to_string(),
        messages,
        backend: Backend::Auto,
        max_tokens: Some(max_tokens),
        temperature: Some(0.1),
        grammar,
        warm_only: false,
    };
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&request).map_err(|error| error.to_string())?
    )
    .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut text = String::new();
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .map_err(|error| format!("inference stream failed: {error}"))?
            == 0
        {
            return Err("inference stream ended before completion".into());
        }
        match serde_json::from_str::<Event>(&line).map_err(|error| error.to_string())? {
            Event::Started { .. } => {}
            Event::Token { text: token, .. } => text.push_str(&token),
            Event::Completed {
                prompt_tokens,
                completion_tokens,
                ..
            } => {
                return Ok(ChatResult {
                    text,
                    prompt_tokens,
                    completion_tokens,
                });
            }
            Event::Error { message, .. } => return Err(message),
            _ => return Err("inference stream returned an unexpected event".into()),
        }
    }
}

fn connect() -> Result<UnixStream, String> {
    let path = ai::runtime_infer_socket_path();
    let stream = UnixStream::connect(&path).map_err(|error| {
        format!(
            "local inference service is unavailable at {}: {error}",
            path.display()
        )
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(180)))
        .map_err(|error| format!("setting inference deadline: {error}"))?;
    Ok(stream)
}

fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
