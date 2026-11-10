//! Scheduler for relay-defined agents.
//!
//! Every tick it looks at the projected definitions, checks each enabled job
//! against its local runtime state, and starts a task through the same
//! `Runner` the routines use. The relay never contains "last run" data, and a
//! failed attempt advances `last_run_at` so an expired grant or missing key
//! retries at the configured cadence instead of every two seconds.

use std::sync::Arc;

use evermore_common::agent::{AgentDefinition, AgentJob, AgentJobRuntime, Task, TaskStatus};
use serde_json::{Value, json};

use crate::agent_runtime::AgentRuntimeStore;
use crate::agents::AgentStore;
use crate::clients;
use crate::{recipe, relay_client, runner::Runner};

pub fn spawn(
    agents: Arc<AgentStore>,
    runtime: Arc<AgentRuntimeStore>,
    base: Arc<Runner>,
    relay_url: String,
) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let now = unix_time();
            for agent in agents.list() {
                if !agent.enabled {
                    continue;
                }
                for job in &agent.jobs {
                    if !job.enabled
                        || !is_due(job, &runtime.job(&agent.id, &job.id), now, agent.created_at)
                    {
                        continue;
                    }
                    match run_job(&agent, job, &base, &relay_url) {
                        Ok(task) => {
                            let _ = runtime.mark_run(&agent.id, &job.id, Some(&task.task_id), None);
                        }
                        Err(error) => {
                            eprintln!(
                                "evermore-agentd: agent '{}' job '{}' did not start: {error}",
                                agent.id, job.id
                            );
                            let _ = runtime.mark_run(&agent.id, &job.id, None, Some(error));
                        }
                    }
                }
            }
        }
    });
}

/// A job is due when it has run before and the interval has elapsed, or — for
/// a newly defined agent — one interval after the definition was created.
/// That makes "every day" mean "starting tomorrow", not "right now"; the
/// agent card's Run now button is the deliberate way to test it immediately.
pub fn is_due(job: &AgentJob, runtime: &AgentJobRuntime, now: u64, created_at: u64) -> bool {
    let baseline = runtime.last_run_at.unwrap_or(created_at);
    now >= baseline.saturating_add(job.interval_seconds)
}

/// Validates the agent, resolves its identity, and starts one task. The
/// caller records the result in `AgentRuntimeStore`.
pub fn run_job(
    agent: &AgentDefinition,
    job: &AgentJob,
    base: &Arc<Runner>,
    relay_url: &str,
) -> Result<Task, String> {
    if !agent.enabled {
        return Err(format!("agent '{}' is paused", agent.id));
    }
    agent.validate()?;
    let agent_pubkey = resolve_agent_pubkey(agent, &base.owner_pubkey, &base.agent_pubkey)?;
    let recipe = recipe::find(&job.recipe_id)?;
    let inputs = if job.inputs.is_null() {
        json!({})
    } else {
        job.inputs.clone()
    };
    let runner = Arc::new(Runner {
        store: Arc::clone(&base.store),
        agent_identity: agent.identity.clone(),
        agent_pubkey,
        owner_pubkey: base.owner_pubkey.clone(),
    });
    let task = runner.create_task(None, &recipe, inputs)?;
    base.store.update(&task.task_id, |task| {
        task.agent_id = Some(agent.id.clone());
        task.job_id = Some(job.id.clone());
        Ok(())
    })?;
    let task = base.store.get(&task.task_id)?;
    let task_id = task.task_id.clone();
    let notify_agent = agent.clone();
    let store = Arc::clone(&base.store);
    let relay_url = relay_url.to_string();
    std::thread::spawn(move || {
        runner.run(task_id.clone(), recipe);
        if let Ok(finished) = store.get(&task_id) {
            notify_finished(&notify_agent, &finished, &relay_url);
        }
    });
    Ok(task)
}

fn notify_finished(agent: &AgentDefinition, task: &Task, relay_url: &str) {
    let summary = task
        .result
        .as_ref()
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .or_else(|| task.error.clone())
        .unwrap_or_else(|| task_status_label(task.status).to_string());
    let summary = summary
        .replace(['\n', '\r'], " ")
        .chars()
        .take(400)
        .collect::<String>();
    let content = format!("{}: {}", agent.title, summary);
    let tags = vec![vec!["t".to_string(), "evermore-notify".to_string()]];
    let Ok(value) = clients::sign_event(&agent.identity, &content, tags) else {
        return;
    };
    let Ok(event) = serde_json::from_value::<nostr::event::Event>(value) else {
        return;
    };
    if let Err(error) = relay_client::publish(relay_url, &event) {
        eprintln!("evermore-agentd: couldn't publish agent notification: {error}");
    }
}

fn task_status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::AwaitingApproval => "waiting for your approval",
        TaskStatus::Succeeded => "finished",
        TaskStatus::Failed => "failed",
        TaskStatus::Denied => "denied",
        TaskStatus::Expired => "expired",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::PartiallyCompleted => "partially completed",
        TaskStatus::ExecutionUnknown => "execution unknown",
        TaskStatus::Replanned => "replanned",
        TaskStatus::Unknown => "unknown",
    }
}

pub fn resolve_agent_pubkey(
    agent: &AgentDefinition,
    owner_pubkey: &str,
    assistant_pubkey: &str,
) -> Result<String, String> {
    if agent.agent_pubkey == owner_pubkey {
        return Err("agent identity must differ from the owner".to_string());
    }
    if agent.agent_pubkey == assistant_pubkey {
        return Err("agent identity duplicates the interactive assistant".to_string());
    }
    let resolved = clients::signer_public_key(&agent.identity)?;
    if resolved != agent.agent_pubkey {
        return Err(format!(
            "identity '{}' resolves to {resolved}, not {}",
            agent.identity, agent.agent_pubkey
        ));
    }
    if resolved == owner_pubkey || resolved == assistant_pubkey {
        return Err("agent identity must be distinct from the owner and assistant".to_string());
    }
    Ok(resolved)
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

    fn job(interval_seconds: u64) -> AgentJob {
        AgentJob {
            id: "digest".into(),
            recipe_id: "audit.digest".into(),
            inputs: json!({}),
            interval_seconds,
            enabled: true,
        }
    }

    #[test]
    fn due_respects_the_interval_and_the_creation_baseline() {
        let job = job(60);
        let runtime = AgentJobRuntime::default();
        // A fresh definition waits one interval instead of running instantly.
        assert!(!is_due(&job, &runtime, 1_000, 1_000));
        assert!(is_due(&job, &runtime, 1_060, 1_000));

        let mut runtime = AgentJobRuntime::default();
        runtime.last_run_at = Some(1_200);
        assert!(!is_due(&job, &runtime, 1_259, 1_000));
        assert!(is_due(&job, &runtime, 1_260, 1_000));
    }

    #[test]
    fn paused_agent_is_refused_before_any_signer_call() {
        let agent = AgentDefinition {
            version: 1,
            id: "morning-digest".into(),
            title: "Morning digest".into(),
            description: String::new(),
            identity: "agent-morning-digest".into(),
            agent_pubkey: "a".repeat(64),
            enabled: false,
            jobs: vec![job(60)],
            created_at: 0,
            updated_at: 0,
        };
        let dir = tempfile::tempdir().unwrap();
        let store =
            Arc::new(crate::store::TaskStore::open_at(dir.path().join("tasks.json")).unwrap());
        let base = Arc::new(Runner {
            store,
            agent_identity: "assistant".into(),
            agent_pubkey: "b".repeat(64),
            owner_pubkey: "c".repeat(64),
        });
        assert!(
            run_job(
                &agent,
                &agent.jobs[0],
                &base,
                relay_client::DEFAULT_RELAY_URL
            )
            .is_err()
        );
    }
}
