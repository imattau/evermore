use std::sync::Arc;

use evermore_common::agent::{StepKind, StepStatus, Task, TaskStatus, TaskStep};
use evermore_common::operations;
use serde_json::{Value, json};

use crate::clients;
use crate::recipe::Recipe;
use crate::render;
use crate::store::TaskStore;

pub struct Runner {
    pub store: Arc<TaskStore>,
    pub agent_pubkey: String,
    pub owner_pubkey: String,
}

impl Runner {
    pub fn create_task(&self, recipe: &Recipe, inputs: Value) -> Result<Task, String> {
        render::validate_inputs(&recipe.inputs_schema(), &inputs)?;
        if recipe.has_catalogue_reads() {
            clients::require_grant(&self.agent_pubkey, &recipe.read_operations())?;
        }
        let now = unix_time();
        let task_id = new_task_id()?;
        let steps = recipe
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| TaskStep {
                index: index as u32,
                id: step.id.clone(),
                kind: step.kind,
                operation: step.operation.clone(),
                arguments: None,
                operation_id: None,
                status: StepStatus::Pending,
                result: None,
                error: None,
                started_at: None,
                finished_at: None,
            })
            .collect();
        let task = Task {
            task_id,
            recipe_id: recipe.id.clone(),
            recipe_version: recipe.version,
            recipe_source: recipe.source.clone(),
            recipe_sha256: recipe.sha256.clone(),
            agent_pubkey: self.agent_pubkey.clone(),
            owner_pubkey: self.owner_pubkey.clone(),
            status: TaskStatus::Pending,
            inputs,
            budget: recipe.budget.clone(),
            steps,
            result: None,
            error: None,
            status_source: None,
            cancel_requested: false,
            created_at: now,
            updated_at: now,
            expires_at: now + recipe.budget.seconds,
        };
        self.store.insert(task)
    }

    pub fn run(self: Arc<Self>, task_id: String, recipe: Recipe) {
        if let Err(error) = self.run_inner(&task_id, &recipe) {
            let _ = self.store.update(&task_id, |task| {
                task.status = if task
                    .steps
                    .iter()
                    .any(|step| step.status == StepStatus::Succeeded)
                {
                    TaskStatus::PartiallyCompleted
                } else {
                    TaskStatus::Failed
                };
                task.error = Some(error);
                Ok(())
            });
        }
    }

    fn run_inner(&self, task_id: &str, recipe: &Recipe) -> Result<(), String> {
        self.store.update(task_id, |task| {
            task.status = TaskStatus::Running;
            Ok(())
        })?;
        let inputs = self.store.get(task_id)?.inputs.clone();
        let mut context = json!({});
        let mut last_render: Option<Value> = None;

        for index in 0..recipe.steps.len() {
            let task = self.store.get(task_id)?;
            if task.cancel_requested || task.status == TaskStatus::Cancelled {
                self.store.update(task_id, |task| {
                    task.status = TaskStatus::Cancelled;
                    Ok(())
                })?;
                return Ok(());
            }
            if unix_time() >= task.expires_at {
                self.store.update(task_id, |task| {
                    task.status = TaskStatus::Expired;
                    task.error = Some("task exceeded its wall-clock budget".into());
                    Ok(())
                })?;
                return Ok(());
            }

            let step = recipe.steps[index].clone();
            self.store.update(task_id, |task| {
                let step = task
                    .steps
                    .get_mut(index)
                    .ok_or_else(|| "task step vanished".to_string())?;
                step.status = StepStatus::Running;
                step.started_at = Some(unix_time());
                Ok(())
            })?;

            let rendered_arguments = match (&step.args, step.kind) {
                (Some(arguments), _) => Some(render::render_value(arguments, &context)?),
                _ => None,
            };

            let result = match step.kind {
                StepKind::Select => {
                    let schema = step
                        .schema
                        .as_ref()
                        .ok_or_else(|| format!("select step '{}' lost its schema", step.id))?;
                    render::validate_inputs(schema, &inputs)?;
                    Ok(inputs.clone())
                }
                StepKind::Journal => {
                    let view = step
                        .view
                        .as_deref()
                        .ok_or_else(|| format!("journal step '{}' lost its view", step.id))?;
                    journal_view(view, step.limit.unwrap_or(50))
                }
                StepKind::Read => {
                    let name = step
                        .operation
                        .as_deref()
                        .ok_or_else(|| format!("read step '{}' lost its operation", step.id))?;
                    clients::require_grant(&self.agent_pubkey, &[name.to_string()])?;
                    let arguments = rendered_arguments.unwrap_or_else(|| json!({}));
                    let value = clients::invoke(name, &arguments)?;
                    let definition = operations::find(name)
                        .ok_or_else(|| format!("unknown operation '{name}'"))?;
                    Ok(operations::redact_result(&definition, value))
                }
                StepKind::Render => {
                    let template = step
                        .template
                        .as_deref()
                        .ok_or_else(|| format!("render step '{}' lost its template", step.id))?;
                    Ok(Value::String(render::render_text(template, &context)?))
                }
                StepKind::Unknown => Err("recipe uses an unknown step kind".into()),
            };

            match result {
                Ok(result) => {
                    context[&step.id] = result.clone();
                    if step.kind == StepKind::Render {
                        last_render = Some(result.clone());
                    }
                    self.store.update(task_id, |task| {
                        let step = task
                            .steps
                            .get_mut(index)
                            .ok_or_else(|| "task step vanished".to_string())?;
                        step.status = StepStatus::Succeeded;
                        step.result = Some(result);
                        step.finished_at = Some(unix_time());
                        Ok(())
                    })?;
                }
                Err(error) => {
                    let completed_before = index > 0;
                    self.store.update(task_id, |task| {
                        let step = task
                            .steps
                            .get_mut(index)
                            .ok_or_else(|| "task step vanished".to_string())?;
                        step.status = StepStatus::Failed;
                        step.error = Some(error.clone());
                        step.finished_at = Some(unix_time());
                        task.status = if completed_before {
                            TaskStatus::PartiallyCompleted
                        } else {
                            TaskStatus::Failed
                        };
                        task.error = Some(error);
                        Ok(())
                    })?;
                    return Ok(());
                }
            }
        }

        self.store.update(task_id, |task| {
            task.status = TaskStatus::Succeeded;
            task.result = last_render.or_else(|| Some(context.clone()));
            Ok(())
        })?;
        Ok(())
    }
}

fn journal_items(read: Result<Vec<Value>, String>) -> Result<Vec<Value>, String> {
    match read {
        Ok(items) => Ok(items),
        Err(error) if error.contains("No such file") => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn journal_view(view: &str, limit: usize) -> Result<Value, String> {
    match view {
        "pending" => {
            let items = journal_items(operations::read_local_pending())?;
            Ok(json!({ "count": items.len(), "items": redact_json(Value::Array(items)) }))
        }
        "history" => {
            let items = journal_items(operations::read_local_history(limit))?;
            Ok(json!({ "count": items.len(), "items": redact_json(Value::Array(items)) }))
        }
        "summary" => {
            let items = journal_items(operations::read_local_history(100))?;
            let mut summary = serde_json::Map::new();
            let mut counts = std::collections::BTreeMap::new();
            for item in &items {
                let status = item
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                *counts.entry(status).or_insert(0_u64) += 1;
            }
            for status in [
                "pending",
                "succeeded",
                "failed",
                "denied",
                "cancelled",
                "execution_unknown",
            ] {
                summary.insert(
                    status.to_string(),
                    Value::from(counts.get(status).copied().unwrap_or(0)),
                );
            }
            summary.insert("total".into(), Value::from(items.len() as u64));
            summary.insert("missing_store".into(), Value::Bool(items.is_empty()));
            Ok(Value::Object(summary))
        }
        other => Err(format!("unknown journal view '{other}'")),
    }
}

fn redact_json(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(evermore_common::ai::redact_text(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_json).collect()),
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| (key, redact_json(value)))
                .collect(),
        ),
        other => other,
    }
}

fn new_task_id() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| format!("couldn't generate task id: {error}"))?;
    Ok(hex::encode(bytes))
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
    use crate::store::TaskStore;

    fn temp_path(label: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "evermore-agentd-runner-{label}-{}-{}",
            std::process::id(),
            unix_time()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("tasks.json")
    }

    fn recipe_with_render() -> Recipe {
        crate::recipe::parse_recipe(
            br#"
id = "test.render"
title = "Render"
budget = { steps = 3, writes = 0, seconds = 10 }

[[step]]
id = "choose"
kind = "select"
schema = { type = "object", properties = { workspace = { type = "integer", minimum = 1, maximum = 9 } }, required = ["workspace"] }

[[step]]
id = "text"
kind = "render"
template = "workspace {{choose.workspace}}"
"#,
            "local",
        )
        .unwrap()
    }

    fn runner(path: std::path::PathBuf) -> Arc<Runner> {
        Arc::new(Runner {
            store: Arc::new(TaskStore::open_at(path).unwrap()),
            agent_pubkey: "a".repeat(64),
            owner_pubkey: "b".repeat(64),
        })
    }

    #[test]
    fn select_and_render_complete_without_inference() {
        let runner = runner(temp_path("render"));
        let recipe = recipe_with_render();
        let task = runner
            .create_task(&recipe, json!({"workspace": 4}))
            .unwrap();
        let task_id = task.task_id.clone();
        Arc::clone(&runner).run(task_id.clone(), recipe);
        let finished = runner.store.get(&task_id).unwrap();
        assert_eq!(finished.status, TaskStatus::Succeeded);
        assert_eq!(finished.result, Some(Value::String("workspace 4".into())));
        assert!(
            finished
                .steps
                .iter()
                .all(|step| step.status == StepStatus::Succeeded)
        );
    }

    #[test]
    fn cancellation_stops_before_the_next_step() {
        let runner = runner(temp_path("cancel"));
        let recipe = recipe_with_render();
        let task = runner
            .create_task(&recipe, json!({"workspace": 4}))
            .unwrap();
        let task_id = task.task_id.clone();
        runner.store.request_cancel(&task_id).unwrap();
        Arc::clone(&runner).run(task_id.clone(), recipe);
        let finished = runner.store.get(&task_id).unwrap();
        assert_eq!(finished.status, TaskStatus::Cancelled);
    }

    #[test]
    fn invalid_inputs_are_refused_before_a_task_exists() {
        let runner = runner(temp_path("inputs"));
        let recipe = recipe_with_render();
        assert!(
            runner
                .create_task(&recipe, json!({"workspace": 40}))
                .is_err()
        );
        assert!(runner.create_task(&recipe, json!({})).is_err());
    }
}
