use std::sync::Arc;

use evermore_common::agent::{self, StepKind, StepStatus, Task, TaskStatus, TaskStep};
use evermore_common::operations;
use serde_json::{Value, json};

use crate::clients;
use crate::recipe::Recipe;
use crate::render;
use crate::store::TaskStore;

pub struct Runner {
    pub store: Arc<TaskStore>,
    pub agent_identity: String,
    pub agent_pubkey: String,
    pub owner_pubkey: String,
}

enum PlannedWrite {
    Single {
        index: usize,
        operation: String,
        arguments: Value,
    },
    Each {
        index: usize,
        operation: String,
        items: Vec<Value>,
    },
}

impl Runner {
    pub fn create_task(
        &self,
        routine_id: Option<String>,
        recipe: &Recipe,
        inputs: Value,
    ) -> Result<Task, String> {
        render::validate_inputs(&recipe.inputs_schema(), &inputs)?;
        let operations = recipe.operation_names();
        if !operations.is_empty() {
            clients::require_grant(&self.agent_pubkey, &operations)?;
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
            routine_id,
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
                let partial = task.steps.iter().any(|step| {
                    step.status == StepStatus::Succeeded || step.operation_id.is_some()
                });
                task.status = if partial {
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
        let mut write_start = None;

        for index in 0..recipe.steps.len() {
            let step = recipe.steps[index].clone();
            if matches!(step.kind, StepKind::Write | StepKind::Each) {
                write_start = Some(index);
                break;
            }
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

            self.store.update(task_id, |task| {
                let step = task
                    .steps
                    .get_mut(index)
                    .ok_or_else(|| "task step vanished".to_string())?;
                step.status = StepStatus::Running;
                step.started_at = Some(unix_time());
                Ok(())
            })?;

            let rendered_arguments = match &step.args {
                Some(arguments) => Some(render::render_value(arguments, &context)?),
                None => None,
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
                StepKind::Write | StepKind::Each => {
                    unreachable!("write steps break before execution")
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

        if let Some(index) = write_start {
            return self.submit_writes(task_id, recipe, index, &context);
        }

        self.store.update(task_id, |task| {
            task.status = TaskStatus::Succeeded;
            task.result = last_render.or_else(|| Some(context.clone()));
            Ok(())
        })?;
        Ok(())
    }

    fn submit_writes(
        &self,
        task_id: &str,
        recipe: &Recipe,
        start: usize,
        context: &Value,
    ) -> Result<(), String> {
        let task = self.store.get(task_id)?;
        if task.cancel_requested || task.status == TaskStatus::Cancelled {
            self.store.update(task_id, |task| {
                task.status = TaskStatus::Cancelled;
                Ok(())
            })?;
            return Ok(());
        }
        clients::require_grant(&self.agent_pubkey, &recipe.write_operations())?;
        let plans = self.plan_writes(recipe, start, context)?;
        let mut submitted = 0_usize;

        for plan in plans {
            let task = self.store.get(task_id)?;
            if task.cancel_requested || task.status == TaskStatus::Cancelled {
                self.store.update(task_id, |task| {
                    task.status = TaskStatus::Cancelled;
                    Ok(())
                })?;
                return Ok(());
            }
            match plan {
                PlannedWrite::Single {
                    index,
                    operation,
                    arguments,
                } => {
                    let operation_id =
                        self.submit_one(task_id, index as u32, &operation, &arguments)?;
                    self.store.update(task_id, |task| {
                        let step = task
                            .steps
                            .get_mut(index)
                            .ok_or_else(|| "task step vanished".to_string())?;
                        step.arguments = Some(arguments);
                        step.operation_id = Some(operation_id);
                        step.status = StepStatus::AwaitingApproval;
                        step.started_at = Some(unix_time());
                        Ok(())
                    })?;
                    submitted += 1;
                }
                PlannedWrite::Each {
                    index,
                    operation,
                    items,
                } => {
                    let placeholder_id = recipe.steps[index].id.clone();
                    let applied = items.len();
                    for (item_index, arguments) in items.into_iter().enumerate() {
                        let next_index = self.store.get(task_id)?.steps.len() as u32;
                        let operation_id =
                            self.submit_one(task_id, next_index, &operation, &arguments)?;
                        let id = format!("{placeholder_id}[{item_index}]");
                        let operation = operation.clone();
                        self.store.update(task_id, |task| {
                            task.steps.push(TaskStep {
                                index: next_index,
                                id,
                                kind: StepKind::Write,
                                operation: Some(operation),
                                arguments: Some(arguments),
                                operation_id: Some(operation_id),
                                status: StepStatus::AwaitingApproval,
                                result: None,
                                error: None,
                                started_at: Some(unix_time()),
                                finished_at: None,
                            });
                            Ok(())
                        })?;
                        submitted += 1;
                    }
                    self.store.update(task_id, |task| {
                        let step = task
                            .steps
                            .get_mut(index)
                            .ok_or_else(|| "task step vanished".to_string())?;
                        step.status = StepStatus::Succeeded;
                        step.result = Some(json!({ "applied": applied }));
                        step.finished_at = Some(unix_time());
                        Ok(())
                    })?;
                }
            }
        }

        self.store.update(task_id, |task| {
            if submitted > 0 {
                task.status = TaskStatus::AwaitingApproval;
            } else {
                task.status = TaskStatus::Succeeded;
                task.result = Some(json!({ "updates": 0 }));
            }
            Ok(())
        })?;
        Ok(())
    }

    fn plan_writes(
        &self,
        recipe: &Recipe,
        start: usize,
        context: &Value,
    ) -> Result<Vec<PlannedWrite>, String> {
        let mut plans = Vec::new();
        let mut total_writes = 0_usize;
        for index in start..recipe.steps.len() {
            let step = &recipe.steps[index];
            let name = step
                .operation
                .as_deref()
                .ok_or_else(|| format!("write step '{}' lost its operation", step.id))?;
            clients::require_grant(&self.agent_pubkey, &[name.to_string()])?;
            match step.kind {
                StepKind::Write => {
                    let rendered = match &step.args {
                        Some(arguments) => render::render_value(arguments, context)?,
                        None => json!({}),
                    };
                    let arguments = operations::canonicalize_input(name, &rendered)?;
                    total_writes += 1;
                    plans.push(PlannedWrite::Single {
                        index,
                        operation: name.to_string(),
                        arguments,
                    });
                }
                StepKind::Each => {
                    let from = step
                        .from
                        .as_deref()
                        .ok_or_else(|| format!("each step '{}' lost its from path", step.id))?;
                    let source = render::resolve_path(context, from).ok_or_else(|| {
                        format!("each step '{}' could not resolve '{from}'", step.id)
                    })?;
                    let items = source.as_array().ok_or_else(|| {
                        format!("each step '{}' source '{from}' is not an array", step.id)
                    })?;
                    let mut rendered_items = Vec::new();
                    for item in items {
                        let mut item_context = context.clone();
                        item_context["item"] = item.clone();
                        let rendered = match &step.args {
                            Some(arguments) => render::render_value(arguments, &item_context)?,
                            None => json!({}),
                        };
                        let arguments = operations::canonicalize_input(name, &rendered)?;
                        total_writes += 1;
                        rendered_items.push(arguments);
                    }
                    plans.push(PlannedWrite::Each {
                        index,
                        operation: name.to_string(),
                        items: rendered_items,
                    });
                }
                _ => {
                    return Err(format!(
                        "recipe step '{}' is not a write or each step",
                        step.id
                    ));
                }
            }
        }
        if total_writes > recipe.budget.writes as usize {
            return Err(format!(
                "write plan needs {total_writes} approvals but the recipe budget allows {}",
                recipe.budget.writes
            ));
        }
        Ok(plans)
    }

    fn submit_one(
        &self,
        task_id: &str,
        step_index: u32,
        operation: &str,
        arguments: &Value,
    ) -> Result<String, String> {
        clients::require_grant(&self.agent_pubkey, &[operation.to_string()])?;
        let plaintext = json!({"operation": operation, "arguments": arguments}).to_string();
        let ciphertext =
            clients::nip44_encrypt(&self.agent_identity, &self.owner_pubkey, &plaintext)?;
        let event = clients::sign_event(
            &self.agent_identity,
            &ciphertext,
            vec![
                vec!["t".into(), "evermore-operation-request".into()],
                vec!["p".into(), self.owner_pubkey.clone()],
                agent::task_tag(task_id),
                agent::step_tag(step_index),
            ],
        )?;
        let record = clients::submit_request(event)?;
        record
            .get("operation_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "operation daemon omitted operation_id".to_string())
    }

    pub fn resume_waiting_tasks(&self) -> Result<(), String> {
        for task in self.store.list(100)? {
            if task.status != TaskStatus::AwaitingApproval {
                continue;
            }
            if task.cancel_requested {
                self.store.update(&task.task_id, |task| {
                    task.status = if task
                        .steps
                        .iter()
                        .any(|step| step.status == StepStatus::Succeeded)
                    {
                        TaskStatus::PartiallyCompleted
                    } else {
                        TaskStatus::Cancelled
                    };
                    Ok(())
                })?;
                continue;
            }
            let mut terminal = true;
            let mut updates = Vec::new();
            for step in &task.steps {
                let Some(operation_id) = &step.operation_id else {
                    continue;
                };
                if step.status != StepStatus::AwaitingApproval {
                    continue;
                }
                match clients::operation_status(operation_id) {
                    Ok(status) => {
                        if !matches!(
                            status.status.as_str(),
                            "succeeded"
                                | "denied"
                                | "expired"
                                | "cancelled"
                                | "failed"
                                | "execution_unknown"
                                | "revoked"
                        ) {
                            terminal = false;
                        }
                        updates.push((step.index, status));
                    }
                    Err(error) => {
                        eprintln!(
                            "evermore-agentd: task {} is waiting; operation status unavailable: {error}",
                            task.task_id
                        );
                        terminal = false;
                    }
                }
            }
            if !terminal {
                continue;
            }
            for (index, status) in updates {
                let (step_status, error) = step_status_from_operation(&status);
                self.store.update(&task.task_id, |task| {
                    let step = task
                        .steps
                        .get_mut(index as usize)
                        .ok_or_else(|| "task step vanished".to_string())?;
                    step.status = step_status;
                    step.result = status.result.clone();
                    step.error = error.clone();
                    step.finished_at = Some(unix_time());
                    Ok(())
                })?;
            }
            self.store.update(&task.task_id, |task| {
                let write_steps: Vec<_> = task
                    .steps
                    .iter()
                    .filter(|step| step.operation_id.is_some())
                    .collect();
                let succeeded = write_steps
                    .iter()
                    .filter(|step| step.status == StepStatus::Succeeded)
                    .count();
                let total = write_steps.len();
                task.status = if succeeded == total {
                    TaskStatus::Succeeded
                } else if succeeded > 0 {
                    TaskStatus::PartiallyCompleted
                } else if write_steps
                    .iter()
                    .all(|step| step.status == StepStatus::Denied)
                {
                    TaskStatus::Denied
                } else if write_steps
                    .iter()
                    .all(|step| step.status == StepStatus::Expired)
                {
                    TaskStatus::Expired
                } else if write_steps
                    .iter()
                    .all(|step| step.status == StepStatus::Cancelled)
                {
                    TaskStatus::Cancelled
                } else {
                    TaskStatus::Failed
                };
                task.result = Some(json!({"writes": total, "succeeded": succeeded}));
                if succeeded != total {
                    task.error = Some(format!(
                        "{} of {total} approved writes completed",
                        succeeded
                    ));
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

fn step_status_from_operation(status: &clients::OperationStatus) -> (StepStatus, Option<String>) {
    match status.status.as_str() {
        "succeeded" => (StepStatus::Succeeded, None),
        "denied" => (StepStatus::Denied, status.error.clone()),
        "expired" => (
            StepStatus::Expired,
            Some("operation request expired before owner approval".into()),
        ),
        "cancelled" => (StepStatus::Cancelled, status.error.clone()),
        "execution_unknown" => (
            StepStatus::ExecutionUnknown,
            Some("operation outcome requires review; it was not replayed".into()),
        ),
        "failed" | "revoked" => (StepStatus::Failed, status.error.clone()),
        other => (
            StepStatus::Failed,
            Some(format!("unexpected operation status '{other}'")),
        ),
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
            agent_identity: "test-agent".into(),
            agent_pubkey: "a".repeat(64),
            owner_pubkey: "b".repeat(64),
        })
    }

    #[test]
    fn select_and_render_complete_without_inference() {
        let runner = runner(temp_path("render"));
        let recipe = recipe_with_render();
        let task = runner
            .create_task(None, &recipe, json!({"workspace": 4}))
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
            .create_task(None, &recipe, json!({"workspace": 4}))
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
                .create_task(None, &recipe, json!({"workspace": 40}))
                .is_err()
        );
        assert!(runner.create_task(None, &recipe, json!({})).is_err());
    }
}
