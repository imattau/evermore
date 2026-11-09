use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Mutex;

use evermore_common::agent::{self, Routine};
use serde::{Deserialize, Serialize};

pub struct RoutineStore {
    path: PathBuf,
    data: Mutex<StoreFile>,
}

#[derive(Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    routines: BTreeMap<String, Routine>,
}

impl RoutineStore {
    pub fn open() -> Result<Self, String> {
        Self::open_at(agent::routines_file_path()?)
    }

    pub(crate) fn open_at(path: PathBuf) -> Result<Self, String> {
        let parent = path
            .parent()
            .ok_or_else(|| "routine store path has no parent directory".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let data: StoreFile = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                format!("couldn't parse routine store {}: {error}", path.display())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreFile::default(),
            Err(error) => return Err(error.to_string()),
        };
        Ok(Self {
            path,
            data: Mutex::new(data),
        })
    }

    pub fn list(&self) -> Result<Vec<Routine>, String> {
        let data = self
            .data
            .lock()
            .map_err(|_| "routine store lock poisoned".to_string())?;
        let mut routines: Vec<Routine> = data.routines.values().cloned().collect();
        routines.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(routines)
    }

    pub fn due(&self, now: u64) -> Result<Vec<Routine>, String> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|routine| routine.enabled)
            .filter(|routine| {
                routine.last_run_at.map_or(true, |last| {
                    now >= last.saturating_add(routine.interval_seconds)
                })
            })
            .collect())
    }

    pub fn add(&self, routine: Routine) -> Result<Routine, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "routine store lock poisoned".to_string())?;
        if data.routines.contains_key(&routine.id) {
            return Err(format!("routine '{}' already exists", routine.id));
        }
        data.routines.insert(routine.id.clone(), routine.clone());
        self.persist(&data)?;
        Ok(routine)
    }

    pub fn set_enabled(&self, routine_id: &str, enabled: bool) -> Result<Routine, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "routine store lock poisoned".to_string())?;
        let routine = data
            .routines
            .get_mut(routine_id)
            .ok_or_else(|| format!("unknown routine '{routine_id}'"))?;
        routine.enabled = enabled;
        let routine = routine.clone();
        self.persist(&data)?;
        Ok(routine)
    }

    pub fn remove(&self, routine_id: &str) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "routine store lock poisoned".to_string())?;
        if data.routines.remove(routine_id).is_none() {
            return Err(format!("unknown routine '{routine_id}'"));
        }
        self.persist(&data)
    }

    pub fn mark_run(
        &self,
        routine_id: &str,
        task_id: Option<String>,
        error: Option<String>,
    ) -> Result<Routine, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "routine store lock poisoned".to_string())?;
        let routine = data
            .routines
            .get_mut(routine_id)
            .ok_or_else(|| format!("unknown routine '{routine_id}'"))?;
        routine.last_run_at = Some(unix_time());
        routine.last_task_id = task_id;
        routine.last_error = error;
        let routine = routine.clone();
        self.persist(&data)?;
        Ok(routine)
    }

    fn persist(&self, data: &StoreFile) -> Result<(), String> {
        let temporary = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(data).map_err(|error| error.to_string())?;
        {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            file.write_all(&bytes).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
        }
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &self.path).map_err(|error| error.to_string())?;
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
    use serde_json::json;

    fn temp_path(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "evermore-agentd-routines-{label}-{}-{}",
            std::process::id(),
            unix_time()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("routines.json")
    }

    fn routine(id: &str, enabled: bool) -> Routine {
        Routine {
            id: id.into(),
            recipe_id: "audit.digest".into(),
            inputs: json!({}),
            interval_seconds: 60,
            agent_identity: format!("{id}-agent"),
            enabled,
            last_run_at: None,
            last_task_id: None,
            last_error: None,
            created_at: unix_time(),
        }
    }

    #[test]
    fn routines_persist_and_due_respects_interval() {
        let path = temp_path("due");
        let store = RoutineStore::open_at(path.clone()).unwrap();
        store.add(routine("digest", true)).unwrap();
        let now = unix_time();
        assert_eq!(store.due(now).unwrap().len(), 1);
        store
            .mark_run("digest", Some("task-1".into()), None)
            .unwrap();
        assert!(store.due(now + 30).unwrap().is_empty());
        assert_eq!(store.due(now + 60).unwrap().len(), 1);
        drop(store);
        let reopened = RoutineStore::open_at(path).unwrap();
        assert_eq!(
            reopened.list().unwrap()[0].last_task_id.as_deref(),
            Some("task-1")
        );
    }

    #[test]
    fn disabled_routines_are_never_due() {
        let store = RoutineStore::open_at(temp_path("disabled")).unwrap();
        store.add(routine("digest", false)).unwrap();
        assert!(store.due(unix_time()).unwrap().is_empty());
        store.set_enabled("digest", true).unwrap();
        assert_eq!(store.due(unix_time()).unwrap().len(), 1);
    }

    #[test]
    fn duplicate_and_unknown_ids_are_refused() {
        let store = RoutineStore::open_at(temp_path("ids")).unwrap();
        store.add(routine("digest", true)).unwrap();
        assert!(store.add(routine("digest", true)).is_err());
        assert!(store.set_enabled("missing", true).is_err());
        assert!(store.remove("missing").is_err());
    }
}
