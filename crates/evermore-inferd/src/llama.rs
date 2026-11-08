//! `llama-server` process pool.
//!
//! `ai-inferd` never links `llama.cpp`. Each loaded base model gets its own
//! child process on an ephemeral loopback port, authenticated with a
//! per-process API key. A crash takes down one child; the next request for
//! that model restarts it. Idle processes and the pool size are bounded.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use evermore_common::ai::Backend;

use crate::http;
use crate::store::InstalledModel;

const STARTUP_POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub addr: SocketAddr,
    pub api_key: String,
    pub backend: Backend,
}

struct Running {
    child: Child,
    endpoint: Endpoint,
    last_used: Instant,
}

pub struct Pool {
    bin: PathBuf,
    max_models: usize,
    ready_timeout: Duration,
    running: Mutex<HashMap<String, Running>>,
}

impl Pool {
    pub fn new(bin: PathBuf, max_models: usize, ready_timeout: Duration) -> Self {
        Self {
            bin,
            max_models: max_models.clamp(1, 8),
            ready_timeout,
            running: Mutex::new(HashMap::new()),
        }
    }

    pub fn ensure(&self, model: &InstalledModel, backend: Backend) -> Result<Endpoint, String> {
        let name = model.name();
        let mut running = self
            .running
            .lock()
            .map_err(|_| "model pool lock is poisoned".to_string())?;

        if let Some(entry) = running.get_mut(&name) {
            let alive = entry.child.try_wait().ok().flatten().is_none();
            if alive && backend_matches(entry.endpoint.backend, backend) {
                entry.last_used = Instant::now();
                return Ok(entry.endpoint.clone());
            }
            if let Some(mut entry) = running.remove(&name) {
                stop(&mut entry);
            }
        }

        while running.len() >= self.max_models {
            let Some(oldest) = running
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(name, _)| name.clone())
            else {
                break;
            };
            if let Some(mut entry) = running.remove(&oldest) {
                eprintln!("evermore-inferd: evicting idle model {oldest}");
                stop(&mut entry);
            }
        }

        let entry = self.spawn(model, backend)?;
        let endpoint = entry.endpoint.clone();
        running.insert(name, entry);
        Ok(endpoint)
    }

    /// Returns the endpoint only if the model's child is already running.
    /// Used by read-only adapter inspection so listing does not start a model.
    pub fn endpoint(&self, name: &str) -> Option<Endpoint> {
        let mut running = self.running.lock().ok()?;
        let alive = {
            let entry = running.get_mut(name)?;
            entry.child.try_wait().ok().flatten().is_none()
        };
        if !alive {
            if let Some(mut entry) = running.remove(name) {
                let _ = entry.child.wait();
            }
            return None;
        }
        running.get(name).map(|entry| entry.endpoint.clone())
    }

    pub fn endpoint_for_backend(&self, name: &str, backend: Backend) -> Option<Endpoint> {
        let endpoint = self.endpoint(name)?;
        backend_matches(endpoint.backend, backend).then_some(endpoint)
    }

    pub fn unload(&self, name: &str) -> bool {
        let Ok(mut running) = self.running.lock() else {
            return false;
        };
        match running.remove(name) {
            Some(mut entry) => {
                stop(&mut entry);
                true
            }
            None => false,
        }
    }

    pub fn shutdown(&self) {
        let Ok(mut running) = self.running.lock() else {
            return;
        };
        for (_, mut entry) in running.drain() {
            stop(&mut entry);
        }
    }

    fn spawn(&self, model: &InstalledModel, backend: Backend) -> Result<Running, String> {
        let addr = SocketAddr::from(([127, 0, 0, 1], free_port()?));
        let api_key = random_key();
        let args = command_args(model, addr.port(), backend, &api_key);
        let log_path = log_path(model);
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| format!("cannot open llama-server log: {error}"))?;
        let log_err = log
            .try_clone()
            .map_err(|error| format!("cannot duplicate llama-server log handle: {error}"))?;

        eprintln!(
            "evermore-inferd: starting {} on {addr} ({backend:?})",
            model.name()
        );
        let mut child = Command::new(&self.bin)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .spawn()
            .map_err(|error| {
                format!(
                    "couldn't start {} ({}): {error}",
                    self.bin.display(),
                    model.name()
                )
            })?;

        let deadline = Instant::now() + self.ready_timeout;
        loop {
            if http::probe_health(addr) {
                return Ok(Running {
                    child,
                    endpoint: Endpoint {
                        addr,
                        api_key,
                        backend,
                    },
                    last_used: Instant::now(),
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!(
                        "llama-server for {} exited during startup ({status}): {}",
                        model.name(),
                        log_tail(&log_path)
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("couldn't poll llama-server: {error}"));
                }
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "llama-server for {} did not become ready within {}s: {}",
                    model.name(),
                    self.ready_timeout.as_secs(),
                    log_tail(&log_path)
                ));
            }
            std::thread::sleep(STARTUP_POLL);
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn backend_matches(running: Backend, requested: Backend) -> bool {
    match requested {
        Backend::Auto => true,
        Backend::Cpu => running == Backend::Cpu,
        Backend::Gpu => running == Backend::Gpu,
    }
}

fn stop(entry: &mut Running) {
    let _ = entry.child.kill();
    let _ = entry.child.wait();
}

/// Pure argument construction so the flag mapping is testable without a
/// `llama.cpp` installation.
fn command_args(model: &InstalledModel, port: u16, backend: Backend, api_key: &str) -> Vec<String> {
    let mut args = vec![
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--model".into(),
        model.base_path.to_string_lossy().into_owned(),
        "--alias".into(),
        model.name(),
        "--ctx-size".into(),
        model.manifest.context_size.to_string(),
        "--no-webui".into(),
        "--api-key".into(),
        api_key.to_string(),
    ];
    match backend {
        Backend::Auto => {}
        Backend::Cpu => {
            args.push("--n-gpu-layers".into());
            args.push("0".into());
        }
        Backend::Gpu => {
            args.push("--n-gpu-layers".into());
            args.push("all".into());
        }
    }
    if model.manifest.supports("embeddings") && !model.manifest.supports("chat") {
        args.push("--embeddings".into());
    }
    if let (Some(adapter), Some(path)) = (&model.manifest.adapter, &model.adapter_path) {
        let path = path.to_string_lossy();
        if (adapter.scale - 1.0).abs() <= f32::EPSILON {
            args.push("--lora".into());
            args.push(path.into_owned());
        } else {
            args.push("--lora-scaled".into());
            args.push(format!("{path}:{}", adapter.scale));
        }
    }
    args
}

fn free_port() -> Result<u16, String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|error| format!("no loopback port: {error}"))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|error| error.to_string())
}

fn random_key() -> String {
    use std::io::Read as _;

    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let mut bytes = [0_u8; 32];
        if file.read_exact(&mut bytes).is_ok() {
            return hex::encode(bytes);
        }
    }
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(std::process::id().to_le_bytes());
    if let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.update(elapsed.as_nanos().to_le_bytes());
    }
    hex::encode(hasher.finalize())
}

fn log_path(model: &InstalledModel) -> PathBuf {
    let base = std::env::var_os("EVERMORE_INFER_LOG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(|dir| PathBuf::from(dir).join("evermore").join("llama-server"))
        })
        .unwrap_or_else(|| std::env::temp_dir().join("evermore-llama-server"));
    let safe: String = model
        .name()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    base.join(format!("{safe}.log"))
}

fn log_tail(path: &Path) -> String {
    const LIMIT: usize = 4096;
    let Ok(bytes) = std::fs::read(path) else {
        return "no log available".into();
    };
    let start = bytes.len().saturating_sub(LIMIT);
    String::from_utf8_lossy(&bytes[start..]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use evermore_common::ai::{MODEL_MANIFEST_VERSION, ModelAdapter, ModelArtifact, ModelManifest};

    fn model(capabilities: &[&str], adapter: Option<ModelAdapter>) -> InstalledModel {
        InstalledModel {
            manifest: ModelManifest {
                manifest_version: MODEL_MANIFEST_VERSION,
                package_id: "p7.model".into(),
                package_version: "1.0.0".into(),
                description: String::new(),
                base: ModelArtifact {
                    file: "base.gguf".into(),
                    sha256: "a".repeat(64),
                    size_bytes: None,
                },
                adapter,
                context_size: 2048,
                quant: "Q4_K_M".into(),
                capabilities: capabilities.iter().map(|value| value.to_string()).collect(),
                operation_registry_version: None,
                training: None,
                publisher: None,
            },
            base_path: PathBuf::from("/models/p7.model-1.0.0/base.gguf"),
            adapter_path: Some(PathBuf::from("/models/p7.model-1.0.0/adapter.gguf")),
        }
    }

    #[test]
    fn cpu_backend_forces_zero_gpu_layers() {
        let args = command_args(&model(&["chat"], None), 1234, Backend::Cpu, "key");
        let index = args.iter().position(|arg| arg == "--n-gpu-layers").unwrap();
        assert_eq!(args[index + 1], "0");
        assert!(args.contains(&"127.0.0.1".to_string()));
        assert!(args.contains(&"--no-webui".to_string()));
    }

    #[test]
    fn gpu_backend_requests_all_layers() {
        let args = command_args(&model(&["chat"], None), 1234, Backend::Gpu, "key");
        let index = args.iter().position(|arg| arg == "--n-gpu-layers").unwrap();
        assert_eq!(args[index + 1], "all");
    }

    #[test]
    fn auto_backend_omits_gpu_flag() {
        let args = command_args(&model(&["chat"], None), 1234, Backend::Auto, "key");
        assert!(!args.contains(&"--n-gpu-layers".to_string()));
    }

    #[test]
    fn embeddings_only_model_gets_embedding_flag() {
        let args = command_args(&model(&["embeddings"], None), 1234, Backend::Auto, "key");
        assert!(args.contains(&"--embeddings".to_string()));
        let chat = command_args(&model(&["chat"], None), 1234, Backend::Auto, "key");
        assert!(!chat.contains(&"--embeddings".to_string()));
    }

    #[test]
    fn adapter_is_passed_with_scale() {
        let adapter = ModelAdapter {
            file: "adapter.gguf".into(),
            sha256: "b".repeat(64),
            size_bytes: None,
            scale: 0.5,
        };
        let args = command_args(&model(&["chat"], Some(adapter)), 1234, Backend::Auto, "key");
        let index = args.iter().position(|arg| arg == "--lora-scaled").unwrap();
        assert_eq!(args[index + 1], "/models/p7.model-1.0.0/adapter.gguf:0.5");
    }

    #[test]
    fn backend_requests_restart_when_they_conflict() {
        assert!(backend_matches(Backend::Cpu, Backend::Auto));
        assert!(backend_matches(Backend::Cpu, Backend::Cpu));
        assert!(!backend_matches(Backend::Auto, Backend::Gpu));
        assert!(!backend_matches(Backend::Cpu, Backend::Gpu));
    }

    #[test]
    fn warm_endpoint_lookup_never_starts_a_model() {
        let pool = Pool::new(PathBuf::from("/does/not/exist"), 1, Duration::from_millis(1));
        assert!(pool.endpoint_for_backend("evermore/not-loaded", Backend::Auto).is_none());
    }
}
