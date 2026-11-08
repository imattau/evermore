//! Talks to `npackd` -- https://github.com/imattau/npack's package-manager
//! daemon, not a component of this repo -- over its documented
//! newline-delimited JSON-RPC Unix socket. The `packages.*` operations
//! used to shell out to `evermore-npack`; that crate is now scoped down to
//! signed AI model (GGUF/LoRA) packages only (see `evermore-inferd`), and
//! general package operations go through the real `npack` here instead.
//! Mirrors `apps/evermore-panel/src/npack_client.rs`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

fn socket_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime).join("npackd.sock")
}

pub fn call(method: &str, params: Value) -> Result<Value, String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|e| {
        format!(
            "couldn't connect to npackd at {} ({e}) -- is it running? (`systemctl --user enable --now npackd.service`)",
            path.display()
        )
    })?;
    let request = json!({"id": 1, "method": method, "params": params});
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
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

pub fn call_typed<T: for<'de> Deserialize<'de>>(method: &str, params: Value) -> Result<T, String> {
    let result = call(method, params)?;
    serde_json::from_value(result).map_err(|e| format!("parsing npackd's {method} result: {e}"))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Dependency {
    #[serde(default)]
    pub publisher: Option<String>,
    pub name: String,
    pub requirement: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InstalledPackage {
    pub publisher: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

pub fn list_installed() -> Result<Vec<InstalledPackage>, String> {
    call_typed("ListInstalled", json!({"user": true}))
}
