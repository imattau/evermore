//! Local, private execution state for relay-defined agent jobs.
//!
//! Definitions live on the relay; when each job last ran, its task id, and
//! its last error belong to this machine. Keeping them out of the relay means
//! a synced definition does not pretend to know what another machine did.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Mutex;

use evermore_common::agent::{self, AgentJobRuntime};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
struct RuntimeFile {
    #[serde(default)]
    agents: BTreeMap<String, BTreeMap<String, AgentJobRuntime>>,
}

pub struct AgentRuntimeStore {
    path: PathBuf,
    data: Mutex<RuntimeFile>,
}

impl AgentRuntimeStore {
    pub fn open() -> Result<Self, String> {
        let path = agent::agent_runtime_file_path()?;
        match Self::open_at(path.clone()) {
            Ok(store) => Ok(store),
            Err(error) => {
                eprintln!("evermore-agentd: ignoring agent runtime state: {error}");
                Ok(Self {
                    path,
                    data: Mutex::new(RuntimeFile::default()),
                })
            }
        }
    }

    pub fn open_at(path: PathBuf) -> Result<Self, String> {
        let data = match std::fs::read(&path) {
            Ok(bytes) if !bytes.is_empty() => serde_json::from_slice::<RuntimeFile>(&bytes)
                .map_err(|error| format!("parsing {}: {error}", path.display()))?,
            Ok(_) => RuntimeFile::default(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => RuntimeFile::default(),
            Err(error) => return Err(format!("reading {}: {error}", path.display())),
        };
        Ok(Self {
            path,
            data: Mutex::new(data),
        })
    }

    pub fn job(&self, agent_id: &str, job_id: &str) -> AgentJobRuntime {
        self.data
            .lock()
            .ok()
            .and_then(|data| {
                data.agents
                    .get(agent_id)
                    .and_then(|jobs| jobs.get(job_id))
                    .cloned()
            })
            .unwrap_or_default()
    }

    pub fn snapshot(&self, agent_id: &str) -> BTreeMap<String, AgentJobRuntime> {
        self.data
            .lock()
            .ok()
            .and_then(|data| data.agents.get(agent_id).cloned())
            .unwrap_or_default()
    }

    /// Records that a job was attempted. `last_run_at` advances even when the
    /// attempt failed, so the scheduler retries after the interval instead of
    /// every tick.
    pub fn mark_run(
        &self,
        agent_id: &str,
        job_id: &str,
        task_id: Option<&str>,
        error: Option<String>,
    ) -> Result<(), String> {
        let data = {
            let mut data = self
                .data
                .lock()
                .map_err(|_| "agent runtime lock poisoned".to_string())?;
            let job = data
                .agents
                .entry(agent_id.to_string())
                .or_default()
                .entry(job_id.to_string())
                .or_default();
            job.last_run_at = Some(unix_time());
            job.last_task_id = task_id.map(str::to_string);
            job.last_error = error;
            (*data).clone()
        };
        self.persist(&data)
    }

    pub fn remove_agent(&self, agent_id: &str) -> Result<(), String> {
        let data = {
            let mut data = self
                .data
                .lock()
                .map_err(|_| "agent runtime lock poisoned".to_string())?;
            if data.agents.remove(agent_id).is_none() {
                return Ok(());
            }
            (*data).clone()
        };
        self.persist(&data)
    }

    fn persist(&self, data: &RuntimeFile) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "agent runtime path has no parent directory".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("setting permissions on {}: {error}", parent.display()))?;

        let temporary = parent.join(format!(".runtime-{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec(data).map_err(|error| error.to_string())?;
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
            std::fs::rename(&temporary, &self.path)
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("writing {}: {error}", self.path.display()));
        }
        Ok(())
    }
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_run_persists_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents/runtime.json");
        let store = AgentRuntimeStore::open_at(path.clone()).unwrap();
        assert!(store.job("a", "j").last_run_at.is_none());

        store
            .mark_run("a", "j", Some("task-1"), Some("boom".to_string()))
            .unwrap();
        let job = store.job("a", "j");
        assert!(job.last_run_at.is_some());
        assert_eq!(job.last_task_id.as_deref(), Some("task-1"));
        assert_eq!(job.last_error.as_deref(), Some("boom"));

        let reopened = AgentRuntimeStore::open_at(path.clone()).unwrap();
        assert_eq!(
            reopened.job("a", "j").last_task_id.as_deref(),
            Some("task-1")
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        store.remove_agent("a").unwrap();
        assert!(store.job("a", "j").last_run_at.is_none());
    }

    #[test]
    fn empty_file_is_a_fresh_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime.json");
        std::fs::write(&path, b"").unwrap();
        let store = AgentRuntimeStore::open_at(path).unwrap();
        assert!(store.snapshot("a").is_empty());
    }
}
