use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Mutex;

use evermore_common::agent::{self, StepStatus, Task, TaskStatus};
use serde::{Deserialize, Serialize};

pub struct TaskStore {
    path: PathBuf,
    data: Mutex<StoreFile>,
}

#[derive(Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    tasks: BTreeMap<String, Task>,
}

impl TaskStore {
    pub fn open() -> Result<Self, String> {
        Self::open_at(agent::tasks_file_path()?)
    }

    pub(crate) fn open_at(path: PathBuf) -> Result<Self, String> {
        let parent = path
            .parent()
            .ok_or_else(|| "task store path has no parent directory".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let mut data: StoreFile = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                format!("couldn't parse task store {}: {error}", path.display())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreFile::default(),
            Err(error) => return Err(error.to_string()),
        };
        let mut recovered = false;
        for task in data.tasks.values_mut() {
            let interrupted = task.status == TaskStatus::Running
                || task
                    .steps
                    .iter()
                    .any(|step| step.status == StepStatus::Running);
            if interrupted {
                task.status = TaskStatus::ExecutionUnknown;
                task.error = Some(
                    "agent daemon restarted after a step was claimed; outcome requires review and must not be replayed"
                        .into(),
                );
                for step in &mut task.steps {
                    if step.status == StepStatus::Running {
                        step.status = StepStatus::ExecutionUnknown;
                        step.error = Some(
                            "step was claimed when the agent daemon stopped; outcome requires review"
                                .into(),
                        );
                    }
                }
                recovered = true;
            }
        }
        let store = Self {
            path,
            data: Mutex::new(data),
        };
        if recovered {
            let data = store.data.lock().map_err(|_| "task store lock poisoned")?;
            store.persist(&data)?;
        }
        Ok(store)
    }

    pub fn insert(&self, task: Task) -> Result<Task, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "task store lock poisoned".to_string())?;
        if data.tasks.contains_key(&task.task_id) {
            return Err("task id already exists".into());
        }
        data.tasks.insert(task.task_id.clone(), task.clone());
        self.persist(&data)?;
        Ok(task)
    }

    pub fn get(&self, task_id: &str) -> Result<Task, String> {
        let data = self
            .data
            .lock()
            .map_err(|_| "task store lock poisoned".to_string())?;
        data.tasks
            .get(task_id)
            .cloned()
            .ok_or_else(|| format!("unknown task '{task_id}'"))
    }

    pub fn list(&self, limit: usize) -> Result<Vec<Task>, String> {
        let data = self
            .data
            .lock()
            .map_err(|_| "task store lock poisoned".to_string())?;
        let mut tasks: Vec<Task> = data.tasks.values().cloned().collect();
        tasks.sort_by_key(|task| std::cmp::Reverse(task.created_at));
        tasks.truncate(limit.clamp(1, 100));
        Ok(tasks)
    }

    pub fn update<F>(&self, task_id: &str, mutate: F) -> Result<Task, String>
    where
        F: FnOnce(&mut Task) -> Result<(), String>,
    {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "task store lock poisoned".to_string())?;
        let snapshot = {
            let task = data
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| format!("unknown task '{task_id}'"))?;
            mutate(task)?;
            task.updated_at = unix_time();
            task.clone()
        };
        self.persist(&data)?;
        Ok(snapshot)
    }

    pub fn request_cancel(&self, task_id: &str) -> Result<Task, String> {
        self.update(task_id, |task| match task.status {
            TaskStatus::Pending | TaskStatus::Running | TaskStatus::AwaitingApproval => {
                task.cancel_requested = true;
                if task.status == TaskStatus::Pending {
                    task.status = TaskStatus::Cancelled;
                }
                Ok(())
            }
            TaskStatus::Succeeded
            | TaskStatus::Failed
            | TaskStatus::Denied
            | TaskStatus::Expired
            | TaskStatus::Cancelled
            | TaskStatus::PartiallyCompleted
            | TaskStatus::ExecutionUnknown
            | TaskStatus::Replanned
            | TaskStatus::Unknown => Err(format!("task is already {}", status_name(task.status))),
        })
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

fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::AwaitingApproval => "awaiting_approval",
        TaskStatus::Succeeded => "succeeded",
        TaskStatus::Failed => "failed",
        TaskStatus::Denied => "denied",
        TaskStatus::Expired => "expired",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::PartiallyCompleted => "partially_completed",
        TaskStatus::ExecutionUnknown => "execution_unknown",
        TaskStatus::Replanned => "replanned",
        TaskStatus::Unknown => "unknown",
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
    use evermore_common::agent::{StepKind, TaskBudget, TaskStep};

    fn temp_path(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "evermore-agentd-{label}-{}-{}",
            std::process::id(),
            unix_time()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("tasks.json")
    }

    fn task(task_id: &str, status: TaskStatus, step_status: StepStatus) -> Task {
        let now = unix_time();
        Task {
            task_id: task_id.into(),
            routine_id: None,
            agent_id: None,
            job_id: None,
            recipe_id: "test.demo".into(),
            recipe_version: 1,
            recipe_source: "builtin".into(),
            recipe_sha256: "builtin".into(),
            agent_pubkey: "a".repeat(64),
            owner_pubkey: "b".repeat(64),
            status,
            inputs: serde_json::json!({}),
            budget: TaskBudget::default(),
            steps: vec![TaskStep {
                index: 0,
                id: "step".into(),
                kind: StepKind::Render,
                operation: None,
                arguments: None,
                operation_id: None,
                status: step_status,
                result: None,
                error: None,
                started_at: None,
                finished_at: None,
            }],
            result: None,
            error: None,
            status_source: None,
            cancel_requested: false,
            created_at: now,
            updated_at: now,
            expires_at: now + 30,
        }
    }

    #[test]
    fn open_recovers_interrupted_tasks_without_replay() {
        let path = temp_path("recover");
        let store = TaskStore::open_at(path.clone()).unwrap();
        store
            .insert(task("one", TaskStatus::Running, StepStatus::Running))
            .unwrap();
        drop(store);
        let reopened = TaskStore::open_at(path).unwrap();
        let recovered = reopened.get("one").unwrap();
        assert_eq!(recovered.status, TaskStatus::ExecutionUnknown);
        assert_eq!(recovered.steps[0].status, StepStatus::ExecutionUnknown);
    }

    #[test]
    fn cancel_is_persisted_and_blocks_pending_tasks() {
        let path = temp_path("cancel");
        let store = TaskStore::open_at(path.clone()).unwrap();
        store
            .insert(task("one", TaskStatus::Pending, StepStatus::Pending))
            .unwrap();
        let cancelled = store.request_cancel("one").unwrap();
        assert!(cancelled.cancel_requested);
        assert_eq!(cancelled.status, TaskStatus::Cancelled);
        drop(store);
        let reopened = TaskStore::open_at(path).unwrap();
        assert_eq!(reopened.get("one").unwrap().status, TaskStatus::Cancelled);
    }
}
