//! Just enough of `npackd`'s -- https://github.com/imattau/npack's
//! package-manager daemon, not a component of this repo -- JSON-RPC Unix
//! socket to resolve an installed GGUF/LoRA model package's version and
//! trusted SHA-256. Mirrors the fuller clients in
//! `apps/evermore-panel/src/npack_client.rs` and
//! `crates/evermore-operationsd/src/npackd_client.rs`.

use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

fn socket_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime).join("npackd.sock")
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstalledPackage {
    pub publisher: String,
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub artifact: PathBuf,
}

pub fn find_installed(publisher: &str, name: &str) -> Result<Option<InstalledPackage>, String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|e| {
        format!(
            "couldn't connect to npackd at {} ({e}) -- is it running? (`systemctl --user enable --now npackd.service`)",
            path.display()
        )
    })?;
    let request = json!({"id": 1, "method": "ListInstalled", "params": {"user": true}});
    writeln!(stream, "{request}").map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let response: Value =
        serde_json::from_str(line.trim()).map_err(|e| format!("parsing npackd's response: {e}"))?;
    if let Some(error) = response.get("error") {
        return Err(error
            .as_str()
            .unwrap_or("npackd returned an error")
            .to_string());
    }
    let installed: Vec<InstalledPackage> =
        serde_json::from_value(response.get("result").cloned().unwrap_or(Value::Null))
            .map_err(|e| format!("parsing npackd's ListInstalled result: {e}"))?;
    Ok(installed
        .into_iter()
        .find(|pkg| pkg.publisher == publisher && pkg.name == name))
}

/// Where npack's own per-user daemon keeps its package store (the retained,
/// verified `.npk` archives, not the unpacked payload -- see
/// `npack_store_root().join("packages")`). Mirrors npack's own
/// `default_store()`: `dirs::data_local_dir()` is `$XDG_DATA_HOME` (falling
/// back to `~/.local/share`) on Linux.
pub fn npack_store_root() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/.local/share")
    });
    PathBuf::from(base).join("npack")
}

/// Where npack unpacks a user-mode install's payload files -- `~/.local`,
/// same as npack's own `install_paths(.., user: true)`. A package's
/// `ai_model.gguf_file` path (declared relative to this prefix, the same
/// way any other payload file in the package is) resolves against this.
pub fn npack_user_prefix() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".local")
}
