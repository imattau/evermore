mod http;
mod llama;
mod store;

use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use evermore_common::ai::{
    self, AdapterState, Backend, Event, Message, ModelAdapter, ModelArtifact, ModelManifest, Role,
};
use serde_json::{Value, json};

const MAX_CHAT_CHARS: usize = 1_000_000;
const DEFAULT_CONTEXT_SIZE: u32 = 8192;

fn main() {
    let result = Config::from_env().and_then(serve);
    if let Err(error) = result {
        eprintln!("evermore-inferd: {error}");
        std::process::exit(1);
    }
}

struct Config {
    server_bin: PathBuf,
    model_roots: Vec<PathBuf>,
    import_root: PathBuf,
    pool: llama::Pool,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let server_bin = std::env::var_os("EVERMORE_LLAMA_SERVER_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("llama-server"));
        let model_roots = ai::model_store_roots();
        if model_roots.is_empty() {
            return Err("no model store roots are configured".into());
        }
        let import_root = std::env::var_os("EVERMORE_MODEL_IMPORT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_import_root);
        let max_models = std::env::var("EVERMORE_INFER_MAX_MODELS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(2);
        let ready_timeout = std::env::var("EVERMORE_INFER_READY_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(120));
        Ok(Self {
            server_bin: server_bin.clone(),
            model_roots,
            import_root,
            pool: llama::Pool::new(server_bin, max_models, ready_timeout),
        })
    }
}

fn default_import_root() -> PathBuf {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    data_home.join("evermore").join("models")
}

fn serve(config: Config) -> Result<(), String> {
    let socket_path = ai::runtime_infer_socket_path();
    let parent = socket_path
        .parent()
        .ok_or_else(|| "inference socket path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())?;
    eprintln!("evermore-inferd listening on {}", socket_path.display());
    for model in store::scan(&config.model_roots) {
        eprintln!(
            "evermore-inferd: model {} ({}, {} ctx)",
            model.name(),
            model.manifest.quant,
            model.manifest.context_size
        );
    }

    let config = Arc::new(config);
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let config = Arc::clone(&config);
                std::thread::spawn(move || {
                    if let Err(error) = handle_connection(stream, &config) {
                        eprintln!("evermore-inferd connection error: {error}");
                    }
                });
            }
            Err(error) => eprintln!("evermore-inferd accept error: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(stream: UnixStream, config: &Config) -> Result<(), String> {
    let writer = stream.try_clone().map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(writer);
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let request: ai::Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                write_event(
                    &mut writer,
                    &Event::Error {
                        request_id: None,
                        message: format!("invalid inference request: {error}"),
                    },
                )?;
                continue;
            }
        };
        let (request_id, result): (Option<String>, Result<Option<Event>, String>) = match request {
            ai::Request::Health => (None, health(config)),
            ai::Request::ListModels => (None, list_models(config)),
            ai::Request::ListAdapters { model } => {
                let result = list_adapters(config, &model).map(Some);
                (None, result)
            }
            ai::Request::SetAdapterScale { model, scale } => {
                let result = set_adapter_scale(config, &model, scale).map(Some);
                (None, result)
            }
            ai::Request::UnloadModel { model } => {
                let result = unload_model(config, &model).map(Some);
                (None, result)
            }
            ai::Request::InstallNpackModel { package_id } => {
                let result = install_npack_model(config, &package_id).map(Some);
                (None, result)
            }
            ai::Request::Embed {
                request_id,
                model,
                inputs,
            } => {
                let result = embed(config, &request_id, &model, &inputs).map(Some);
                (Some(request_id), result)
            }
            ai::Request::Chat {
                request_id,
                model,
                messages,
                backend,
                max_tokens,
                temperature,
                grammar,
            } => {
                if let Err(error) = validate_chat(
                    &request_id,
                    &model,
                    &messages,
                    max_tokens,
                    temperature,
                    grammar.as_deref(),
                ) {
                    (Some(request_id), Err(error))
                } else {
                    let result = chat(
                        config,
                        &mut writer,
                        &request_id,
                        &model,
                        &messages,
                        backend,
                        max_tokens,
                        temperature,
                        grammar.as_deref(),
                    )
                    .map(|()| None);
                    (Some(request_id), result)
                }
            }
        };
        match result {
            Ok(Some(event)) => write_event(&mut writer, &event)?,
            Ok(None) => {}
            Err(message) => write_event(
                &mut writer,
                &Event::Error {
                    request_id,
                    message,
                },
            )?,
        }
    }
    Ok(())
}

fn health(config: &Config) -> Result<Option<Event>, String> {
    Ok(Some(Event::Health {
        protocol_version: ai::PROTOCOL_VERSION,
        backend: "llama.cpp".into(),
        ready: binary_available(&config.server_bin),
    }))
}

fn binary_available(bin: &Path) -> bool {
    if bin.components().count() > 1 {
        return bin.is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(bin).is_file())
}

fn list_models(config: &Config) -> Result<Option<Event>, String> {
    Ok(Some(Event::Models {
        models: models(config),
    }))
}

fn models(config: &Config) -> Vec<ai::Model> {
    store::scan(&config.model_roots)
        .into_iter()
        .map(|model| model.to_public_model())
        .collect()
}

fn unload_model(config: &Config, model: &str) -> Result<Event, String> {
    validate_model_name(model)?;
    config.pool.unload(model);
    Ok(Event::Models {
        models: models(config),
    })
}

fn list_adapters(config: &Config, model: &str) -> Result<Event, String> {
    validate_model_name(model)?;
    let installed = store::find(&config.model_roots, model)?;
    let Some(declared) = installed.manifest.adapter.as_ref() else {
        return Ok(Event::Adapters {
            model: model.to_string(),
            adapters: Vec::new(),
        });
    };
    if let Some(endpoint) = config.pool.endpoint(model) {
        let adapters = runtime_adapters(&endpoint)?;
        if adapters.is_empty() {
            return Err(format!(
                "llama-server did not load the adapter declared by model '{model}'"
            ));
        }
        return Ok(Event::Adapters {
            model: model.to_string(),
            adapters,
        });
    }
    Ok(Event::Adapters {
        model: model.to_string(),
        adapters: vec![AdapterState {
            id: 0,
            name: declared.file.clone(),
            scale: declared.scale,
            loaded: false,
        }],
    })
}

fn set_adapter_scale(config: &Config, model: &str, scale: f32) -> Result<Event, String> {
    validate_model_name(model)?;
    validate_adapter_scale(scale)?;
    let installed = store::find(&config.model_roots, model)?;
    if installed.manifest.adapter.is_none() {
        return Err(format!("model '{model}' has no signed LoRA adapter"));
    }
    let endpoint = config.pool.ensure(&installed, Backend::Auto)?;
    let mut response = http::open_response(
        endpoint.addr,
        Some(&endpoint.api_key),
        "POST",
        "/lora-adapters",
        Some(&json!([{ "id": 0, "scale": scale }])),
    )?;
    if !(200..300).contains(&response.status) {
        let mut text = String::new();
        response
            .body
            .read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        return Err(format!(
            "llama-server rejected the adapter scale (HTTP {}): {text}",
            response.status
        ));
    }
    let adapters = runtime_adapters(&endpoint)?;
    if adapters.is_empty() {
        return Err(format!(
            "llama-server did not report the adapter for model '{model}'"
        ));
    }
    Ok(Event::Adapters {
        model: model.to_string(),
        adapters,
    })
}

fn validate_adapter_scale(scale: f32) -> Result<(), String> {
    if !scale.is_finite() || !(0.0..=4.0).contains(&scale) {
        return Err("adapter scale must be between 0 and 4".into());
    }
    Ok(())
}

fn runtime_adapters(endpoint: &llama::Endpoint) -> Result<Vec<AdapterState>, String> {
    let response = http::request_json(
        endpoint.addr,
        Some(&endpoint.api_key),
        "GET",
        "/lora-adapters",
        None,
    )?;
    let items = response
        .as_array()
        .ok_or_else(|| "llama-server returned an invalid adapter list".to_string())?;
    Ok(items
        .iter()
        .enumerate()
        .map(|(position, item)| {
            let path = item.get("path").and_then(Value::as_str).unwrap_or_default();
            AdapterState {
                id: item
                    .get("id")
                    .and_then(Value::as_u64)
                    .unwrap_or(position as u64) as u32,
                name: Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "adapter".into()),
                scale: item.get("scale").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                loaded: true,
            }
        })
        .collect())
}

fn validate_model_name(model: &str) -> Result<(), String> {
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_whitespace) {
        return Err("model must be an installed model name".into());
    }
    if model.ends_with(":cloud") {
        return Err("cloud models are disabled in evermore-inferd".into());
    }
    Ok(())
}

fn validate_chat(
    request_id: &str,
    model: &str,
    messages: &[Message],
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    grammar: Option<&str>,
) -> Result<(), String> {
    if request_id.is_empty() || request_id.len() > 128 {
        return Err("request_id must contain between 1 and 128 characters".into());
    }
    validate_model_name(model)?;
    if messages.is_empty() || messages.len() > 64 {
        return Err("messages must contain between 1 and 64 entries".into());
    }
    if !messages.iter().any(|message| message.role == Role::User) {
        return Err("messages must include at least one user message".into());
    }
    let total_chars: usize = messages
        .iter()
        .map(|message| message.content.chars().count())
        .sum();
    if total_chars == 0 || total_chars > MAX_CHAT_CHARS {
        return Err("combined message content must be between 1 character and 1 MB".into());
    }
    if max_tokens.is_some_and(|tokens| tokens == 0 || tokens > 32_768) {
        return Err("max_tokens must be between 1 and 32768".into());
    }
    if temperature.is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value)) {
        return Err("temperature must be between 0 and 2".into());
    }
    if let Some(grammar) = grammar {
        validate_grammar(grammar)?;
    }
    Ok(())
}

fn validate_grammar(grammar: &str) -> Result<(), String> {
    if grammar.trim().is_empty() {
        return Err("grammar must not be empty".into());
    }
    if grammar.len() > 64 * 1024 {
        return Err("grammar must be at most 64 KiB".into());
    }
    if grammar.contains('\0') {
        return Err("grammar must not contain NUL bytes".into());
    }
    Ok(())
}

fn embed(
    config: &Config,
    request_id: &str,
    model: &str,
    inputs: &[String],
) -> Result<Event, String> {
    if request_id.is_empty() || request_id.len() > 128 {
        return Err("request_id must contain between 1 and 128 characters".into());
    }
    validate_model_name(model)?;
    if inputs.is_empty() || inputs.len() > 128 {
        return Err("embedding inputs must contain between 1 and 128 items".into());
    }
    let total_chars: usize = inputs.iter().map(|input| input.chars().count()).sum();
    if total_chars == 0 || total_chars > MAX_CHAT_CHARS {
        return Err("combined embedding input must be between 1 character and 1 MB".into());
    }
    let installed = store::find(&config.model_roots, model)?;
    if !installed.manifest.supports("embeddings") {
        return Err(format!(
            "model '{model}' does not declare the embeddings capability; install a signed embedding model package"
        ));
    }
    let endpoint = config.pool.ensure(&installed, Backend::Auto)?;
    let started = Instant::now();
    let result = http::request_json(
        endpoint.addr,
        Some(&endpoint.api_key),
        "POST",
        "/v1/embeddings",
        Some(&json!({ "model": installed.name(), "input": inputs })),
    )?;
    let data = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "llama-server omitted embedding vectors".to_string())?;
    let mut indexed = Vec::with_capacity(data.len());
    for (position, item) in data.iter().enumerate() {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(position as u64);
        let vector: Vec<f32> = serde_json::from_value(
            item.get("embedding")
                .cloned()
                .ok_or_else(|| "llama-server omitted an embedding vector".to_string())?,
        )
        .map_err(|error| format!("invalid local embedding vectors: {error}"))?;
        indexed.push((index, vector));
    }
    indexed.sort_by_key(|(index, _)| *index);
    let vectors: Vec<Vec<f32>> = indexed.into_iter().map(|(_, vector)| vector).collect();
    if vectors.len() != inputs.len()
        || vectors.is_empty()
        || vectors[0].is_empty()
        || vectors.iter().any(|vector| {
            vector.len() != vectors[0].len() || vector.iter().any(|value| !value.is_finite())
        })
    {
        return Err("local embedding backend returned inconsistent vectors".into());
    }
    Ok(Event::Embeddings {
        request_id: request_id.to_string(),
        model: model.to_string(),
        vectors,
        prompt_tokens: result
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64),
        total_duration_ns: Some(started.elapsed().as_nanos().min(u64::MAX as u128) as u64),
    })
}

#[allow(clippy::too_many_arguments)]
fn chat(
    config: &Config,
    writer: &mut impl Write,
    request_id: &str,
    model: &str,
    messages: &[Message],
    backend: Backend,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    grammar: Option<&str>,
) -> Result<(), String> {
    let installed = store::find(&config.model_roots, model)?;
    if !installed.manifest.supports("chat") {
        return Err(format!(
            "model '{model}' does not declare the chat capability; install a chat model package"
        ));
    }
    let endpoint = config.pool.ensure(&installed, backend)?;

    write_event(
        writer,
        &Event::Started {
            request_id: request_id.to_string(),
            model: model.to_string(),
            backend_requested: backend,
        },
    )
    .map_err(|error| error.to_string())?;

    let messages: Vec<Value> = messages
        .iter()
        .map(|message| {
            json!({
                "role": match message.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                },
                "content": message.content
            })
        })
        .collect();
    let mut body = serde_json::Map::new();
    body.insert("model".into(), json!(installed.name()));
    body.insert("messages".into(), json!(messages));
    body.insert("stream".into(), json!(true));
    body.insert("stream_options".into(), json!({ "include_usage": true }));
    if let Some(max_tokens) = max_tokens {
        body.insert("max_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(grammar) = grammar {
        body.insert("grammar".into(), json!(grammar));
    }

    let started = Instant::now();
    let mut response = http::open_response(
        endpoint.addr,
        Some(&endpoint.api_key),
        "POST",
        "/v1/chat/completions",
        Some(&Value::Object(body)),
    )?;
    if !(200..300).contains(&response.status) {
        let mut text = String::new();
        response
            .body
            .read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        return Err(format!(
            "llama-server returned HTTP {}: {text}",
            response.status
        ));
    }

    let mut reader = BufReader::new(response.body);
    let mut prompt_tokens = None;
    let mut completion_tokens = None;
    let mut completed = false;
    let mut line = String::new();
    loop {
        line.clear();
        let count = reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            completed = true;
            break;
        }
        if data.is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(data)
            .map_err(|error| format!("invalid llama-server stream record: {error}"))?;
        if let Some(error) = record.get("error") {
            let message = error
                .as_str()
                .map(str::to_string)
                .or_else(|| {
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| error.to_string());
            return Err(message);
        }
        if let Some(text) = record
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            write_event(
                writer,
                &Event::Token {
                    request_id: request_id.to_string(),
                    text: text.to_string(),
                },
            )
            .map_err(|error| error.to_string())?;
        }
        if let Some(usage) = record.get("usage") {
            prompt_tokens = usage.get("prompt_tokens").and_then(Value::as_u64);
            completion_tokens = usage.get("completion_tokens").and_then(Value::as_u64);
        }
        if record
            .pointer("/choices/0/finish_reason")
            .is_some_and(|reason| !reason.is_null())
        {
            completed = true;
        }
    }
    if !completed {
        return Err("local inference stream ended before completion".into());
    }
    write_event(
        writer,
        &Event::Completed {
            request_id: request_id.to_string(),
            backend_used: endpoint.backend,
            prompt_tokens,
            completion_tokens,
            total_duration_ns: Some(started.elapsed().as_nanos().min(u64::MAX as u128) as u64),
        },
    )
    .map_err(|error| error.to_string())
}

fn install_npack_model(config: &Config, package_id: &str) -> Result<Event, String> {
    if package_id.is_empty()
        || package_id.len() > 128
        || !package_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
    {
        return Err("package_id contains characters unsafe for an install path".into());
    }
    let root = evermore_npack::install_root().join(package_id);
    let current = std::fs::canonicalize(root.join("current"))
        .map_err(|error| format!("package is not installed: {error}"))?;
    let canonical_package = std::fs::canonicalize(&root).map_err(|error| error.to_string())?;
    if !current.starts_with(&canonical_package) {
        return Err("installed package current link escapes its package directory".into());
    }
    let manifest: evermore_npack::Manifest = serde_json::from_slice(
        &std::fs::read(current.join("manifest.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("invalid installed package manifest: {error}"))?;
    if manifest.id != package_id {
        return Err("installed manifest package id does not match requested id".into());
    }
    let payload = manifest
        .ai_model
        .clone()
        .ok_or("installed package does not declare an AI model payload")?;
    let base_relative = std::path::Path::new(&payload.gguf_file);
    if base_relative.as_os_str().is_empty()
        || base_relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || payload.sha256.len() != 64
        || !payload.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("model payload path or SHA-256 is malformed".into());
    }
    let base_source = std::fs::canonicalize(current.join(base_relative))
        .map_err(|error| format!("model payload is missing: {error}"))?;
    if !base_source.starts_with(&current) || !base_source.is_file() {
        return Err("model payload resolves outside the installed package".into());
    }
    let adapter_source = match (&payload.adapter_gguf_file, &payload.adapter_sha256) {
        (Some(file), Some(hash)) => {
            let relative = std::path::Path::new(file);
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
                || hash.len() != 64
                || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err("adapter payload path or SHA-256 is malformed".into());
            }
            let path = std::fs::canonicalize(current.join(relative))
                .map_err(|error| format!("adapter payload is missing: {error}"))?;
            if !path.starts_with(&current) || !path.is_file() {
                return Err("adapter payload resolves outside the installed package".into());
            }
            Some((path, hash.to_ascii_lowercase()))
        }
        (None, None) => None,
        _ => return Err("adapter payload must declare both a file and a SHA-256".into()),
    };
    let release_bytes = std::fs::read(current.join("evermore-release-event.json"))
        .map_err(|error| format!("signed release evidence is missing: {error}"))?;
    let release: nostr::prelude::Event = serde_json::from_slice(&release_bytes)
        .map_err(|error| format!("invalid signed release evidence: {error}"))?;
    release
        .verify()
        .map_err(|error| format!("signed release verification failed: {error}"))?;
    if release.kind.as_u16() != evermore_npack::KIND_RELEASE_ARTIFACT_SET
        || !release.tags.iter().any(|tag| {
            let values = tag.as_slice();
            values.first().is_some_and(|name| name == "d")
                && values.get(1).is_some_and(|value| value == package_id)
        })
        || !release.tags.iter().any(|tag| {
            let values = tag.as_slice();
            values.first().is_some_and(|name| name == "version")
                && values
                    .get(1)
                    .is_some_and(|value| value == &manifest.version)
        })
    {
        return Err("signed release does not bind this installed package version".into());
    }
    let archive_hash = release
        .tags
        .iter()
        .find_map(|tag| {
            let values = tag.as_slice();
            (values.first().is_some_and(|name| name == "x"))
                .then(|| values.get(1))
                .flatten()
        })
        .ok_or("signed release has no archive hash")?;
    let archive_path = current.join("package.npk");
    if !archive_path.exists() {
        return Err("installed archive is unavailable for release binding verification".into());
    }
    if verify_file_sha256(&archive_path, archive_hash)? != archive_hash.to_ascii_lowercase() {
        return Err("installed package archive does not match signed release hash".into());
    }
    if verify_file_sha256(&base_source, &payload.sha256)? != payload.sha256.to_ascii_lowercase() {
        return Err("GGUF payload hash does not match installed manifest".into());
    }
    if let Some((path, expected)) = &adapter_source
        && verify_file_sha256(path, expected)? != *expected
    {
        return Err("adapter GGUF hash does not match installed manifest".into());
    }

    let capabilities = if payload.capabilities.is_empty() {
        vec!["chat".to_string()]
    } else {
        payload.capabilities.clone()
    };
    let model_manifest = ModelManifest {
        manifest_version: ai::MODEL_MANIFEST_VERSION,
        package_id: package_id.to_string(),
        package_version: manifest.version.clone(),
        description: manifest.name.clone(),
        base: ModelArtifact {
            file: "base.gguf".into(),
            sha256: payload.sha256.to_ascii_lowercase(),
            size_bytes: std::fs::metadata(&base_source)
                .ok()
                .map(|metadata| metadata.len()),
        },
        adapter: adapter_source.as_ref().map(|(_, hash)| ModelAdapter {
            file: "adapter.gguf".into(),
            sha256: hash.clone(),
            size_bytes: None,
            scale: payload.adapter_scale.unwrap_or(1.0),
        }),
        context_size: payload.context_size.unwrap_or(DEFAULT_CONTEXT_SIZE),
        quant: payload.quant.clone().unwrap_or_default(),
        capabilities,
        operation_registry_version: payload.operation_registry_version.clone(),
        publisher: Some(release.pubkey.to_hex()),
    };
    model_manifest.validate()?;

    std::fs::create_dir_all(&config.import_root)
        .map_err(|error| format!("cannot create model store: {error}"))?;
    let directory_name = format!(
        "{}-{}",
        safe_dir_name(package_id),
        safe_dir_name(&manifest.version)
    );
    let destination = config.import_root.join(&directory_name);
    let staging = config.import_root.join(format!(
        ".{directory_name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("cannot create model staging directory: {error}"))?;

    let result = (|| -> Result<(), String> {
        link_or_copy(&base_source, &staging.join("base.gguf"))?;
        if let Some((source, _)) = &adapter_source {
            link_or_copy(source, &staging.join("adapter.gguf"))?;
        }
        let mut staged_manifest = model_manifest.clone();
        if let Some(adapter) = staged_manifest.adapter.as_mut() {
            adapter.size_bytes = std::fs::metadata(staging.join("adapter.gguf"))
                .ok()
                .map(|metadata| metadata.len());
        }
        std::fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&staged_manifest).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("cannot write model manifest: {error}"))?;
        if destination.exists() {
            std::fs::remove_dir_all(&destination)
                .map_err(|error| format!("cannot replace existing model: {error}"))?;
        }
        std::fs::rename(&staging, &destination)
            .map_err(|error| format!("cannot publish model: {error}"))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    let installed = store::InstalledModel {
        manifest: model_manifest,
        base_path: destination.join("base.gguf"),
        adapter_path: adapter_source.map(|_| destination.join("adapter.gguf")),
    };
    Ok(Event::ModelInstalled {
        package_id: package_id.to_string(),
        model: installed.to_public_model(),
    })
}

fn safe_dir_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn link_or_copy(source: &Path, destination: &Path) -> Result<(), String> {
    if std::fs::hard_link(source, destination).is_ok() {
        return Ok(());
    }
    std::fs::copy(source, destination)
        .map(|_| ())
        .map_err(|error| format!("cannot copy model artifact: {error}"))
}

fn verify_file_sha256(path: &Path, expected: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let actual = hex::encode(digest.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        eprintln!(
            "evermore-inferd: hash mismatch for {}: expected {expected}, got {actual}",
            path.display()
        );
    }
    Ok(actual)
}

fn write_event(writer: &mut impl Write, event: &Event) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, event).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_limits_are_enforced() {
        validate_grammar("root ::= \"ok\"").unwrap();
        assert!(validate_grammar("   ").is_err());
        assert!(validate_grammar("root\0 ::= \"x\"").is_err());
        assert!(validate_grammar(&"a".repeat(64 * 1024 + 1)).is_err());
    }

    #[test]
    fn adapter_scale_limits_are_enforced() {
        validate_adapter_scale(0.0).unwrap();
        validate_adapter_scale(1.0).unwrap();
        validate_adapter_scale(4.0).unwrap();
        assert!(validate_adapter_scale(-0.1).is_err());
        assert!(validate_adapter_scale(4.1).is_err());
        assert!(validate_adapter_scale(f32::NAN).is_err());
    }

    #[test]
    fn chat_accepts_an_optional_grammar() {
        let messages = vec![Message {
            role: Role::User,
            content: "emit an operation".into(),
        }];
        validate_chat(
            "req",
            "evermore/m:1",
            &messages,
            Some(16),
            Some(0.0),
            Some("root ::= \"{}\""),
        )
        .unwrap();
        assert!(validate_chat("req", "evermore/m:1", &messages, Some(16), Some(0.0), None).is_ok());
    }
}
