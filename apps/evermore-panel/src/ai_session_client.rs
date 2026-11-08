use evermore_common::ai::Message;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextScope {
    #[default]
    Turn,
    Pinned,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshPolicy {
    #[default]
    Snapshot,
    EachTurn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextGrant {
    pub source: String,
    #[serde(default)]
    pub scope: ContextScope,
    #[serde(default)]
    pub refresh: RefreshPolicy,
    #[serde(default)]
    pub snapshot: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub updated_at: u64,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub context: Vec<ContextGrant>,
}

pub fn load() -> Result<Vec<Session>, String> {
    let path = sessions_path();
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("reading local AI sessions: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("reading local AI sessions: {error}")),
    }
}

pub fn save(sessions: &[Session]) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let path = sessions_path();
    let parent = path
        .parent()
        .ok_or_else(|| "AI session path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    serde_json::to_writer_pretty(&mut file, sessions).map_err(|error| error.to_string())?;
    std::io::Write::write_all(&mut file, b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        error.to_string()
    })
}

fn sessions_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    base.join("evermore/ai-sessions.json")
}
