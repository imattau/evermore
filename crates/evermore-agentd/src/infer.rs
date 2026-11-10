use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use evermore_common::ai::{self, Backend, Event, Message, Model, Request};

pub struct ChatResult {
    pub text: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

/// Pick the panel proposal adapter.
pub fn select_chat_model() -> Result<String, String> {
    select_task_model("panel")
}

/// Select the adapter trained for a specific runtime task. Task-specific
/// models are never silently substituted with another adapter: doing so can
/// turn a valid MCP call into a panel proposal (or vice versa).
pub fn select_task_model(task: &str) -> Result<String, String> {
    let (env, package_hint) = match task {
        "panel" => ("EVERMORE_PANEL_MODEL", ".panel"),
        "mcp" => ("EVERMORE_MCP_MODEL", ".mcp"),
        _ => return Err(format!("unknown model task '{task}'")),
    };
    let models = list_models()?;
    if let Ok(configured) = std::env::var(env) {
        let configured = configured.trim();
        if configured.is_empty() {
            return Err(format!("{env} must name an installed model"));
        }
        return models
            .iter()
            .find(|model| model.name == configured && supports_chat(model))
            .map(|model| model.name.clone())
            .ok_or_else(|| {
                format!("configured {task} model '{configured}' is not an installed chat model")
            });
    }
    if let Some(model) = models
        .iter()
        .find(|model| model.name.contains(package_hint) && supports_chat(model))
    {
        return Ok(model.name.clone());
    }
    if task == "mcp" {
        return Err(
            "no MCP adapter is installed; set EVERMORE_MCP_MODEL to an installed MCP chat model"
                .into(),
        );
    }
    Err(format!(
        "no {task} adapter is installed; install the Evermore {task} model or set {env} to its installed model name"
    ))
}

fn supports_chat(model: &Model) -> bool {
    model.capabilities.is_empty()
        || model
            .capabilities
            .iter()
            .any(|capability| capability == "chat")
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
