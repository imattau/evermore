use evermore_common::{ai, ai_index};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
};

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const CHUNK_CHARS: usize = 1200;
const OVERLAP: usize = 160;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Chunk {
    scope_id: String,
    path: String,
    text: String,
    vector: Vec<f32>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    chunks: Vec<Chunk>,
}

fn main() {
    if let Err(e) = serve() {
        eprintln!("evermore-indexd: {e}");
        std::process::exit(1);
    }
}

fn serve() -> Result<(), String> {
    let socket = ai_index::runtime_socket_path();
    let parent = socket.parent().ok_or("index socket has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    eprintln!("evermore-indexd listening on {}", socket.display());
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                std::thread::spawn(|| {
                    if let Err(e) = handle(stream) {
                        eprintln!("index connection: {e}");
                    }
                });
            }
            Err(e) => eprintln!("index accept: {e}"),
        }
    }
    Ok(())
}

fn handle(stream: UnixStream) -> Result<(), String> {
    let mut writer = BufWriter::new(stream.try_clone().map_err(|e| e.to_string())?);
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let response = match serde_json::from_str::<ai_index::Request>(&line) {
            Ok(req) => execute(req),
            Err(e) => Err(format!("invalid index request: {e}")),
        };
        let event = match response {
            Ok(event) => event,
            Err(message) => ai_index::Event::Error { message },
        };
        serde_json::to_writer(&mut writer, &event).map_err(|e| e.to_string())?;
        writer.write_all(b"\n").map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn execute(req: ai_index::Request) -> Result<ai_index::Event, String> {
    match req {
        ai_index::Request::Health => Ok(ai_index::Event::Health {
            protocol_version: ai_index::PROTOCOL_VERSION,
            ready: true,
        }),
        ai_index::Request::ListScopes => Ok(ai_index::Event::Scopes { scopes: scopes()? }),
        ai_index::Request::Reindex { scope_id } => {
            let scope = scopes()?
                .into_iter()
                .find(|s| s.id == scope_id)
                .ok_or("unknown explicitly configured scope")?;
            let path = std::path::PathBuf::from(&scope.path);
            let chunks = index_file(&scope.id, &path)?;
            let count = chunks.len();
            let mut index = load_index()?;
            index
                .chunks
                .retain(|c| c.scope_id != scope.id && c.path != scope.path);
            index.chunks.extend(chunks);
            save_index(&index)?;
            Ok(ai_index::Event::Indexed {
                scope_id,
                documents: 1,
                chunks: count,
            })
        }
        ai_index::Request::RemoveScope { scope_id } => {
            let scope = scopes()?.into_iter().find(|s| s.id == scope_id);
            let mut index = load_index()?;
            if let Some(scope) = scope {
                index
                    .chunks
                    .retain(|c| c.scope_id != scope_id && c.path != scope.path);
            } else {
                index.chunks.retain(|c| c.scope_id != scope_id);
            }
            save_index(&index)?;
            Ok(ai_index::Event::Removed { scope_id })
        }
        ai_index::Request::Search { query, limit } => {
            let query = query.trim();
            if query.is_empty() || query.len() > 8192 {
                return Err("query must contain 1 to 8192 bytes".into());
            }
            let limit = limit.unwrap_or(5).clamp(1, 20);
            let query = ai::redact_text(query);
            let vector = embed(vec![query.clone()])?.remove(0);
            let index = load_index()?;
            let allow: std::collections::HashSet<String> =
                scopes()?.into_iter().map(|s| s.id).collect();
            let mut results: Vec<_> = index
                .chunks
                .into_iter()
                .filter(|c| allow.contains(&c.scope_id))
                .map(|c| {
                    let score = cosine(&vector, &c.vector);
                    ai_index::SearchResult {
                        scope_id: c.scope_id,
                        path: c.path,
                        excerpt: c.text,
                        score,
                    }
                })
                .filter(|r| r.score.is_finite())
                .collect();
            results.sort_by(|a, b| b.score.total_cmp(&a.score));
            results.truncate(limit);
            Ok(ai_index::Event::Results {
                query: query.into(),
                results,
            })
        }
    }
}

fn scopes() -> Result<Vec<ai_index::Scope>, String> {
    let raw = match std::env::var("EVERMORE_INDEX_FILES") {
        Ok(raw) => raw,
        Err(_) => {
            let path = ai_index::config_file_path();
            if path.is_file() {
                std::fs::read_to_string(path)
                    .map_err(|e| format!("reading index selection: {e}"))?
            } else {
                "[]".to_string()
            }
        }
    };
    let paths: Vec<String> = serde_json::from_str(&raw).map_err(|e| {
        format!("EVERMORE_INDEX_FILES must be a JSON array of selected file paths: {e}")
    })?;
    let mut out = Vec::new();
    for path in paths {
        let p = std::path::PathBuf::from(&path);
        let meta = std::fs::symlink_metadata(&p)
            .map_err(|e| format!("selected file {}: {e}", p.display()))?;
        if !meta.file_type().is_file() {
            continue;
        }
        let id = format!("{:x}-{:x}", meta.dev(), meta.ino());
        out.push(ai_index::Scope {
            id,
            path: p
                .canonicalize()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
        });
    }
    Ok(out)
}

fn index_file(scope_id: &str, path: &std::path::Path) -> Result<Vec<Chunk>, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() || meta.len() > MAX_FILE_BYTES {
        return Err("selected path must be a regular file no larger than 2 MiB".into());
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["md", "txt", "rst", "org"].contains(&ext.as_str()) {
        return Err("only md, txt, rst and org files can be indexed".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000); // O_NOFOLLOW
    }
    let file = options
        .open(path)
        .map_err(|e| format!("opening selected file without following links: {e}"))?;
    let mut content = String::new();
    use std::io::Read;
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|e| format!("reading selected file: {e}"))?;
    if content.len() as u64 > MAX_FILE_BYTES {
        return Err("selected file grew beyond 2 MiB while being read".into());
    }
    let content = ai::redact_text(&content);
    let parts = chunks(&content);
    if parts.is_empty() {
        return Ok(vec![]);
    }
    let mut vectors = Vec::with_capacity(parts.len());
    for batch in parts.chunks(128) {
        vectors.extend(embed(batch.to_vec())?);
    }
    if vectors.len() != parts.len() {
        return Err("embedding count did not match document chunks".into());
    }
    Ok(parts
        .into_iter()
        .zip(vectors)
        .map(|(text, vector)| Chunk {
            scope_id: scope_id.into(),
            path: path.to_string_lossy().into_owned(),
            text,
            vector,
        })
        .collect())
}

fn chunks(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + CHUNK_CHARS).min(chars.len());
        let piece: String = chars[start..end].iter().collect();
        let safe = ai::redact_text(&piece);
        if !safe.trim().is_empty() {
            out.push(safe);
        }
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(OVERLAP);
    }
    out
}

fn resolve_embed_model() -> Result<String, String> {
    if let Ok(model) = std::env::var("EVERMORE_EMBED_MODEL")
        && !model.trim().is_empty()
    {
        return Ok(model);
    }
    let mut stream = UnixStream::connect(ai::runtime_infer_socket_path())
        .map_err(|e| format!("connecting to local inference daemon: {e}"))?;
    serde_json::to_writer(&mut stream, &ai::Request::ListModels).map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let event: ai::Event =
        serde_json::from_str(&line).map_err(|e| format!("invalid model-list response: {e}"))?;
    match event {
        ai::Event::Models { models } => models
            .into_iter()
            .find(|model| {
                model
                    .capabilities
                    .iter()
                    .any(|capability| capability == "embeddings")
            })
            .map(|model| model.name)
            .ok_or_else(|| {
                "no embedding model is installed; install a signed npack embedding package or set EVERMORE_EMBED_MODEL".into()
            }),
        ai::Event::Error { message, .. } => Err(message),
        _ => Err("unexpected model-list response".into()),
    }
}

fn embed(inputs: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
    let model = resolve_embed_model()?;
    if model.ends_with(":cloud") {
        return Err("cloud embedding models are disabled".into());
    }
    let request_id = format!(
        "index-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    );
    let req = ai::Request::Embed {
        request_id: request_id.clone(),
        model,
        inputs,
    };
    let mut stream = UnixStream::connect(ai::runtime_infer_socket_path())
        .map_err(|e| format!("connecting to local inference daemon: {e}"))?;
    serde_json::to_writer(&mut stream, &req).map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let event: ai::Event =
        serde_json::from_str(&line).map_err(|e| format!("invalid embedding response: {e}"))?;
    match event {
        ai::Event::Embeddings {
            request_id: got,
            vectors,
            ..
        } if got == request_id => Ok(vectors),
        ai::Event::Error { message, .. } => Err(message),
        _ => Err("unexpected embedding response".into()),
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return f32::NAN;
    }
    let (mut dot, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa == 0.0 || bb == 0.0 {
        f32::NAN
    } else {
        dot / (aa.sqrt() * bb.sqrt())
    }
}

fn index_path() -> Result<std::path::PathBuf, String> {
    let root = std::env::var("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| {
            std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state"))
        })
        .map_err(|e| e.to_string())?
        .join("evermore");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    Ok(root.join("semantic-index.json"))
}
fn load_index() -> Result<Index, String> {
    let p = index_path()?;
    if !p.exists() {
        return Ok(Index::default());
    }
    serde_json::from_slice(&std::fs::read(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
fn save_index(index: &Index) -> Result<(), String> {
    let p = index_path()?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(index).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    std::fs::rename(tmp, p).map_err(|e| e.to_string())
}
