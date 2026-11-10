use std::collections::{BTreeMap, BTreeSet};

use evermore_common::agent::{
    self, MAX_LOCAL_RECIPE_BYTES, MAX_RECIPE_SECONDS, MAX_RECIPE_STEPS, RecipeSummary, StepKind,
    TaskBudget,
};
use evermore_common::operations::{self, Access};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct Recipe {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub description: String,
    pub operations: Vec<String>,
    pub budget: TaskBudget,
    pub steps: Vec<RecipeStep>,
    pub source: String,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct RecipeStep {
    pub id: String,
    pub kind: StepKind,
    pub operation: Option<String>,
    pub args: Option<Value>,
    pub schema: Option<Value>,
    pub template: Option<String>,
    pub view: Option<String>,
    pub limit: Option<usize>,
    /// Array path for an `each` step, for example `plan.updates`.
    pub from: Option<String>,
}

impl Recipe {
    pub fn summary(&self) -> RecipeSummary {
        RecipeSummary {
            id: self.id.clone(),
            version: self.version,
            title: self.title.clone(),
            description: self.description.clone(),
            source: self.source.clone(),
            operations: self.operations.clone(),
            budget: self.budget.clone(),
            inputs_schema: self.inputs_schema(),
        }
    }

    pub fn inputs_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for step in &self.steps {
            if step.kind != StepKind::Select {
                continue;
            }
            let Some(schema) = &step.schema else {
                continue;
            };
            if let Some(object) = schema.get("properties").and_then(Value::as_object) {
                for (name, property) in object {
                    properties.insert(name.clone(), property.clone());
                }
            }
            if let Some(names) = schema.get("required").and_then(Value::as_array) {
                for name in names {
                    if let Some(name) = name.as_str() {
                        required.push(Value::String(name.to_string()));
                    }
                }
            }
        }
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    pub fn has_catalogue_reads(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.kind == StepKind::Read && step.operation.is_some())
    }

    pub fn read_operations(&self) -> Vec<String> {
        self.steps
            .iter()
            .filter(|step| step.kind == StepKind::Read)
            .filter_map(|step| step.operation.clone())
            .collect()
    }

    pub fn write_operations(&self) -> Vec<String> {
        self.steps
            .iter()
            .filter(|step| matches!(step.kind, StepKind::Write | StepKind::Each))
            .filter_map(|step| step.operation.clone())
            .collect()
    }

    pub fn operation_names(&self) -> Vec<String> {
        self.steps
            .iter()
            .filter(|step| matches!(step.kind, StepKind::Read | StepKind::Write | StepKind::Each))
            .filter_map(|step| step.operation.clone())
            .collect()
    }

    pub fn write_count(&self) -> u32 {
        self.steps
            .iter()
            .filter(|step| matches!(step.kind, StepKind::Write | StepKind::Each))
            .count() as u32
    }
}

#[derive(Debug, Deserialize)]
struct RawRecipe {
    id: String,
    #[serde(default = "default_version")]
    version: u32,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    operations: Vec<String>,
    #[serde(default)]
    budget: TaskBudget,
    #[serde(default, rename = "step")]
    steps: Vec<RawStep>,
}

#[derive(Debug, Deserialize)]
struct RawStep {
    id: String,
    kind: RawStepKind,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    args: Option<toml::Value>,
    #[serde(default)]
    schema: Option<toml::Value>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    from: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawStepKind {
    Select,
    Read,
    Journal,
    Render,
    Write,
    Each,
}

fn default_version() -> u32 {
    1
}

pub fn audit_digest() -> Recipe {
    Recipe {
        id: "audit.digest".into(),
        version: 1,
        title: "Audit digest".into(),
        description:
            "Summarise recorded operations and current pending requests from the private local journal. Read-only; no catalogue write and no model call."
                .into(),
        operations: Vec::new(),
        budget: TaskBudget {
            steps: 2,
            writes: 0,
            seconds: 15,
            context_tokens: 0,
        },
        steps: vec![
            RecipeStep {
                id: "summary".into(),
                kind: StepKind::Journal,
                operation: None,
                args: None,
                schema: None,
                template: None,
                view: Some("summary".into()),
                limit: None,
                from: None,
            },
            RecipeStep {
                id: "digest".into(),
                kind: StepKind::Render,
                operation: None,
                args: None,
                schema: None,
                template: Some(
                    "Audit digest\nPending requests: {{summary.pending}}\nSucceeded: {{summary.succeeded}}\nFailed: {{summary.failed}}\nDenied: {{summary.denied}}\nCancelled: {{summary.cancelled}}\nExecution unknown: {{summary.execution_unknown}}\nTotal recorded: {{summary.total}}"
                        .into(),
                ),
                view: None,
                limit: None,
                from: None,
            },
        ],
        source: "builtin".into(),
        sha256: "builtin".into(),
    }
}

pub fn builtins() -> Vec<Recipe> {
    vec![
        audit_digest(),
        builtin(include_str!("../recipes/settings.concierge.toml")),
        builtin(include_str!("../recipes/workspace.compose.toml")),
        builtin(include_str!("../recipes/notifications.dnd.toml")),
        builtin(include_str!("../recipes/packages.doctor.toml")),
        builtin(include_str!("../recipes/packages.updates.toml")),
        builtin_ns(
            include_str!("../nscript/recipes/desktop.check.ns"),
            "desktop.check",
        ),
        builtin_ns(
            include_str!("../nscript/recipes/updates.watch.ns"),
            "updates.watch",
        ),
        builtin_ns(include_str!("../nscript/recipes/ai.check.ns"), "ai.check"),
        builtin_ns(
            include_str!("../nscript/recipes/notifications.triage.ns"),
            "notifications.triage",
        ),
        builtin_ns(
            include_str!("../nscript/recipes/updates.install.ns"),
            "updates.install",
        ),
        builtin_ns(
            include_str!("../nscript/recipes/workspace.prep.ns"),
            "workspace.prep",
        ),
    ]
}

fn builtin(text: &str) -> Recipe {
    parse_recipe(text.as_bytes(), "builtin").expect("built-in recipe must be valid")
}

fn builtin_ns(source: &str, id: &str) -> Recipe {
    crate::nscript_recipe::parse_ns_recipe(source.as_bytes(), "builtin", id)
        .expect("built-in nscript recipe must be valid")
}

/// Load built-ins plus every valid local recipe. Invalid local recipes are
/// skipped with a diagnostic on stderr; `load_local` reports the specific
/// error for tests and CLI diagnostics.
pub fn load_all() -> Vec<Recipe> {
    let mut recipes = builtins();
    let Ok(directory) = agent::recipes_dir() else {
        return recipes;
    };
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return recipes;
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("toml" | "ns")
            )
        })
        .collect();
    paths.sort();
    for path in paths {
        match load_local(&path) {
            Ok(recipe) => {
                if recipes.iter().any(|existing| existing.id == recipe.id) {
                    eprintln!(
                        "evermore-agentd: local recipe {} reuses built-in id '{}'; ignored",
                        path.display(),
                        recipe.id
                    );
                    continue;
                }
                recipes.push(recipe);
            }
            Err(error) => {
                eprintln!(
                    "evermore-agentd: local recipe {} ignored: {error}",
                    path.display()
                );
            }
        }
    }
    recipes
}

pub fn find(id: &str) -> Result<Recipe, String> {
    load_all()
        .into_iter()
        .find(|recipe| recipe.id == id)
        .ok_or_else(|| format!("unknown recipe '{id}'"))
}

pub fn load_local(path: &std::path::Path) -> Result<Recipe, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("couldn't read recipe metadata: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("recipe must be a regular, non-symlink file".into());
    }
    if metadata.len() > MAX_LOCAL_RECIPE_BYTES {
        return Err(format!(
            "recipe is larger than {} bytes",
            MAX_LOCAL_RECIPE_BYTES
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err("recipe must not be group- or world-writable; run chmod 600 on it".into());
        }
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if path.extension().and_then(|ext| ext.to_str()) == Some("ns") {
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("recipe file name is not valid UTF-8")?;
        return crate::nscript_recipe::parse_ns_recipe(&bytes, "local", stem);
    }
    parse_recipe(&bytes, "local")
}

pub fn parse_recipe(bytes: &[u8], source: &str) -> Result<Recipe, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|error| format!("recipe is not UTF-8: {error}"))?;
    let raw: RawRecipe =
        toml::from_str(text).map_err(|error| format!("recipe is not valid TOML: {error}"))?;
    let recipe = Recipe {
        id: raw.id,
        version: raw.version,
        title: raw.title,
        description: raw.description,
        operations: raw.operations,
        budget: raw.budget,
        steps: raw
            .steps
            .into_iter()
            .map(|step| RecipeStep {
                id: step.id,
                kind: match step.kind {
                    RawStepKind::Select => StepKind::Select,
                    RawStepKind::Read => StepKind::Read,
                    RawStepKind::Journal => StepKind::Journal,
                    RawStepKind::Render => StepKind::Render,
                    RawStepKind::Write => StepKind::Write,
                    RawStepKind::Each => StepKind::Each,
                },
                operation: step.operation,
                args: step.args.map(toml_to_json),
                schema: step.schema.map(toml_to_json),
                template: step.template,
                view: step.view,
                limit: step.limit,
                from: step.from,
            })
            .collect(),
        source: source.to_string(),
        sha256: hex::encode(Sha256::digest(bytes)),
    };
    validate_recipe(&recipe)?;
    Ok(recipe)
}

fn toml_to_json(value: toml::Value) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

pub(crate) fn validate_recipe(recipe: &Recipe) -> Result<(), String> {
    validate_id(&recipe.id)?;
    if recipe.version == 0 {
        return Err("recipe version must be at least 1".into());
    }
    if recipe.title.trim().is_empty() || recipe.title.len() > 200 {
        return Err("recipe title must be 1-200 characters".into());
    }
    if recipe.description.len() > 2000 {
        return Err("recipe description is longer than 2000 characters".into());
    }
    validate_budget(&recipe.budget)?;
    if recipe.steps.is_empty() {
        return Err("recipe has no steps".into());
    }
    if recipe.steps.len() as u32 > recipe.budget.steps {
        return Err("recipe has more steps than its budget allows".into());
    }
    if recipe.steps.len() as u32 > MAX_RECIPE_STEPS {
        return Err("recipe exceeds the maximum step count".into());
    }

    let declared: BTreeSet<&str> = recipe.operations.iter().map(String::as_str).collect();
    if declared.len() != recipe.operations.len() {
        return Err("recipe declares a duplicate operation".into());
    }
    for name in &recipe.operations {
        let definition = operations::find(name)
            .ok_or_else(|| format!("recipe declares unknown operation '{name}'"))?;
        match definition.access {
            Access::Read => {
                if definition.approval == operations::Approval::Required {
                    return Err(format!(
                        "'{name}' unexpectedly requires approval for a read"
                    ));
                }
            }
            Access::Write => {
                if definition.approval != operations::Approval::Required {
                    return Err(format!(
                        "'{name}' is a write operation without owner approval"
                    ));
                }
            }
        }
    }

    let mut step_ids = BTreeSet::new();
    let mut select_properties: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut seen_write = false;
    let mut each_index = None;
    for (index, step) in recipe.steps.iter().enumerate() {
        validate_id(&step.id)?;
        if !step_ids.insert(step.id.clone()) {
            return Err(format!("duplicate step id '{}'", step.id));
        }
        if matches!(step.kind, StepKind::Write | StepKind::Each) {
            seen_write = true;
        } else if seen_write {
            return Err(
                "write steps must be the final steps of a recipe so the owner can review the whole write plan"
                    .into(),
            );
        }
        match step.kind {
            StepKind::Select => {
                let schema = step
                    .schema
                    .as_ref()
                    .ok_or_else(|| format!("select step '{}' needs a schema", step.id))?;
                let properties = schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .ok_or_else(|| format!("select step '{}' schema has no properties", step.id))?;
                let mut names = BTreeSet::new();
                for (name, property) in properties {
                    validate_slot_schema(name, property)?;
                    if select_properties
                        .values()
                        .any(|existing| existing.contains(name))
                    {
                        return Err(format!("duplicate select slot '{name}'"));
                    }
                    names.insert(name.clone());
                }
                if let Some(required) = schema.get("required").and_then(Value::as_array) {
                    for name in required {
                        let name = name.as_str().ok_or_else(|| {
                            "select required list must contain strings".to_string()
                        })?;
                        if !names.contains(name) {
                            return Err(format!(
                                "select step '{}' requires undeclared slot '{name}'",
                                step.id
                            ));
                        }
                    }
                }
                select_properties.insert(step.id.clone(), names);
            }
            StepKind::Read => {
                let name = step
                    .operation
                    .as_deref()
                    .ok_or_else(|| format!("read step '{}' needs an operation", step.id))?;
                if !declared.contains(name) {
                    return Err(format!(
                        "read step '{}' uses operation '{name}' not declared in operations",
                        step.id
                    ));
                }
                if let Some(args) = &step.args {
                    if !args.is_object() {
                        return Err(format!("read step '{}' args must be a table", step.id));
                    }
                }
            }
            StepKind::Journal => {
                let view = step
                    .view
                    .as_deref()
                    .ok_or_else(|| format!("journal step '{}' needs a view", step.id))?;
                if !["pending", "history", "summary"].contains(&view) {
                    return Err(format!(
                        "journal step '{}' has unknown view '{view}'",
                        step.id
                    ));
                }
                if step.limit.is_some_and(|limit| !(1..=100).contains(&limit)) {
                    return Err(format!(
                        "journal step '{}' limit must be between 1 and 100",
                        step.id
                    ));
                }
            }
            StepKind::Render => {
                let template = step
                    .template
                    .as_deref()
                    .ok_or_else(|| format!("render step '{}' needs a template", step.id))?;
                if template.is_empty() || template.len() > 64 * 1024 {
                    return Err(format!(
                        "render step '{}' template must be 1-65536 bytes",
                        step.id
                    ));
                }
            }
            StepKind::Write => {
                let name = step
                    .operation
                    .as_deref()
                    .ok_or_else(|| format!("write step '{}' needs an operation", step.id))?;
                if !declared.contains(name) {
                    return Err(format!(
                        "write step '{}' uses operation '{name}' not declared in operations",
                        step.id
                    ));
                }
                let definition = operations::find(name)
                    .ok_or_else(|| format!("write step '{}' has an unknown operation", step.id))?;
                if definition.access != Access::Write
                    || definition.approval != operations::Approval::Required
                {
                    return Err(format!(
                        "write step '{}' operation '{name}' is not an approval-gated write",
                        step.id
                    ));
                }
                if let Some(args) = &step.args {
                    if !args.is_object() {
                        return Err(format!("write step '{}' args must be a table", step.id));
                    }
                }
            }
            StepKind::Each => {
                if each_index.is_some() {
                    return Err("a recipe may contain at most one each step".into());
                }
                each_index = Some(index);
                let name = step
                    .operation
                    .as_deref()
                    .ok_or_else(|| format!("each step '{}' needs an operation", step.id))?;
                if !declared.contains(name) {
                    return Err(format!(
                        "each step '{}' uses operation '{name}' not declared in operations",
                        step.id
                    ));
                }
                let definition = operations::find(name)
                    .ok_or_else(|| format!("each step '{}' has an unknown operation", step.id))?;
                if definition.access != Access::Write
                    || definition.approval != operations::Approval::Required
                {
                    return Err(format!(
                        "each step '{}' operation '{name}' is not an approval-gated write",
                        step.id
                    ));
                }
                let from = step
                    .from
                    .as_deref()
                    .ok_or_else(|| format!("each step '{}' needs a from path", step.id))?;
                if from.contains("{{") || from.contains("}}") {
                    return Err(format!(
                        "each step '{}' from must be a plain path, not a template",
                        step.id
                    ));
                }
                validate_reference(from, index, &recipe.steps, &select_properties, false)?;
                if let Some(args) = &step.args {
                    if !args.is_object() {
                        return Err(format!("each step '{}' args must be a table", step.id));
                    }
                }
            }
            StepKind::Unknown => return Err("recipe uses an unknown step kind".into()),
        }

        for reference in step_references(step) {
            validate_reference(
                &reference,
                index,
                &recipe.steps,
                &select_properties,
                step.kind == StepKind::Each,
            )?;
        }
    }
    if each_index.is_some_and(|index| index + 1 != recipe.steps.len()) {
        return Err("each must be the final step of a recipe".into());
    }
    let writes = recipe.write_count();
    if writes > 0 && recipe.budget.writes == 0 {
        return Err("recipe has write steps but a zero write budget".into());
    }
    if writes > recipe.budget.writes {
        return Err("recipe has more write steps than its write budget allows".into());
    }
    Ok(())
}

fn validate_budget(budget: &TaskBudget) -> Result<(), String> {
    if budget.steps == 0 || budget.steps > MAX_RECIPE_STEPS {
        return Err(format!(
            "step budget must be between 1 and {MAX_RECIPE_STEPS}"
        ));
    }
    if budget.writes > MAX_RECIPE_STEPS {
        return Err("write budget exceeds the maximum step count".into());
    }
    if budget.seconds == 0 || budget.seconds > MAX_RECIPE_SECONDS {
        return Err(format!(
            "time budget must be between 1 and {MAX_RECIPE_SECONDS} seconds"
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 {
        return Err("identifier must be 1-128 characters".into());
    }
    if !id.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || "._-".contains(character)
    }) {
        return Err(format!(
            "identifier '{id}' must be lowercase ASCII letters, digits, '.', '_' or '-'"
        ));
    }
    Ok(())
}

fn validate_slot_schema(name: &str, property: &Value) -> Result<(), String> {
    validate_id(name)?;
    let object = property
        .as_object()
        .ok_or_else(|| format!("slot '{name}' schema must be an object"))?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("slot '{name}' schema needs a type"))?;
    if !["string", "integer", "boolean"].contains(&kind) {
        return Err(format!(
            "slot '{name}' type must be string, integer, or boolean"
        ));
    }
    if let Some(values) = object.get("enum") {
        let valid = values.as_array().is_some_and(|values| {
            !values.is_empty()
                && values.iter().all(|value| match kind {
                    "string" => value.is_string(),
                    "integer" => value.as_i64().is_some(),
                    "boolean" => value.as_bool().is_some(),
                    _ => false,
                })
        });
        if !valid {
            return Err(format!("slot '{name}' has an invalid enum"));
        }
    }
    Ok(())
}

fn step_references(step: &RecipeStep) -> Vec<String> {
    let mut references = Vec::new();
    collect_value_references(step.args.as_ref(), &mut references);
    collect_value_references(step.schema.as_ref(), &mut references);
    if let Some(template) = &step.template {
        collect_text_references(template, &mut references);
    }
    references
}

fn collect_value_references(value: Option<&Value>, output: &mut Vec<String>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::String(text) => collect_text_references(text, output),
        Value::Array(items) => {
            for item in items {
                collect_value_references(Some(item), output);
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_value_references(Some(value), output);
            }
        }
        _ => {}
    }
}

fn collect_text_references(text: &str, output: &mut Vec<String>) {
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let reference = after[..end].trim();
        if !reference.is_empty() {
            output.push(reference.to_string());
        }
        rest = &after[end + 2..];
    }
}

fn validate_reference(
    reference: &str,
    current_index: usize,
    steps: &[RecipeStep],
    select_properties: &BTreeMap<String, BTreeSet<String>>,
    allow_item: bool,
) -> Result<(), String> {
    if allow_item && (reference == "item" || reference.starts_with("item.")) {
        return Ok(());
    }
    let mut parts = reference.split('.');
    let step_id = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| format!("invalid template reference '{{{{{reference}}}}}'"))?;
    let Some(step_index) = steps
        .iter()
        .take(current_index)
        .position(|step| step.id == step_id)
    else {
        return Err(format!(
            "template reference '{{{{{reference}}}}}' must point at an earlier step"
        ));
    };
    let step = &steps[step_index];
    let field = parts.next();
    match step.kind {
        StepKind::Select => {
            let Some(field) = field else {
                return Err(format!(
                    "template reference '{{{{{reference}}}}}' must name a select slot"
                ));
            };
            if !select_properties
                .get(&step.id)
                .is_some_and(|properties| properties.contains(field))
            {
                return Err(format!(
                    "template reference '{{{{{reference}}}}}' names an undeclared slot"
                ));
            }
        }
        StepKind::Journal => {
            let Some(field) = field else {
                return Err(format!(
                    "template reference '{{{{{reference}}}}}' must name a journal field"
                ));
            };
            let view = step.view.as_deref().unwrap_or("");
            let allowed: &[&str] = match view {
                "summary" => &[
                    "pending",
                    "succeeded",
                    "failed",
                    "denied",
                    "cancelled",
                    "execution_unknown",
                    "total",
                    "missing_store",
                ],
                "pending" | "history" => &["count", "items"],
                _ => &[],
            };
            if !allowed.contains(&field) {
                return Err(format!(
                    "template reference '{{{{{reference}}}}}' is not available for journal view '{view}'"
                ));
            }
        }
        StepKind::Read | StepKind::Render | StepKind::Write | StepKind::Each => {}
        StepKind::Unknown => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
id = "test.demo"
version = 1
title = "Demo"
operations = ["settings.get"]

budget = { steps = 3, writes = 0, seconds = 10 }

[[step]]
id = "value"
kind = "read"
operation = "settings.get"
args = { domain = "theme", key = "mode" }

[[step]]
id = "text"
kind = "render"
template = "Theme: {{value.value}}"
"#;

    #[test]
    fn parses_a_valid_recipe() {
        let recipe = parse_recipe(VALID.as_bytes(), "local").unwrap();
        assert_eq!(recipe.id, "test.demo");
        assert_eq!(recipe.steps.len(), 2);
        assert!(recipe.has_catalogue_reads());
        assert_ne!(recipe.sha256, "builtin");
    }

    const WRITE: &str = r#"
id = "test.write"
version = 1
title = "Write demo"
operations = ["settings.set"]
budget = { steps = 3, writes = 1, seconds = 10 }

[[step]]
id = "choose"
kind = "select"
schema = { type = "object", properties = { value = { type = "string" } }, required = ["value"] }

[[step]]
id = "text"
kind = "render"
template = "Theme: {{choose.value}}"

[[step]]
id = "apply"
kind = "write"
operation = "settings.set"
args = { domain = "theme", key = "mode", value = "{{choose.value}}" }
"#;

    #[test]
    fn parses_a_valid_write_recipe() {
        let recipe = parse_recipe(WRITE.as_bytes(), "local").unwrap();
        assert_eq!(recipe.write_count(), 1);
        assert_eq!(recipe.write_operations(), vec!["settings.set"]);
    }

    #[test]
    fn rejects_a_write_without_write_budget() {
        let text = WRITE.replace("writes = 1", "writes = 0");
        let error = parse_recipe(text.as_bytes(), "local").unwrap_err();
        assert!(error.contains("zero write budget"), "{error}");
    }

    #[test]
    fn rejects_a_non_write_operation_as_a_write_step() {
        let text = WRITE
            .replace(
                "operations = [\"settings.set\"]",
                "operations = [\"settings.get\"]",
            )
            .replace(
                "operation = \"settings.set\"",
                "operation = \"settings.get\"",
            )
            .replace("writes = 1", "writes = 1");
        let error = parse_recipe(text.as_bytes(), "local").unwrap_err();
        assert!(error.contains("not an approval-gated write"), "{error}");
    }

    #[test]
    fn rejects_a_write_before_a_later_non_write_step() {
        let text = WRITE.replace(
            "[[step]]\nid = \"text\"\nkind = \"render\"\ntemplate = \"Theme: {{choose.value}}\"\n\n[[step]]\nid = \"apply\"",
            "[[step]]\nid = \"apply\"",
        );
        let text = text.replace(
            "args = { domain = \"theme\", key = \"mode\", value = \"{{choose.value}}\" }",
            "args = { domain = \"theme\", key = \"mode\", value = \"{{choose.value}}\" }\n\n[[step]]\nid = \"text\"\nkind = \"render\"\ntemplate = \"Theme: {{choose.value}}\"",
        );
        let error = parse_recipe(text.as_bytes(), "local").unwrap_err();
        assert!(error.contains("final steps"), "{error}");
    }

    #[test]
    fn rejects_undeclared_operation_use() {
        let text = VALID.replace("operations = [\"settings.get\"]", "operations = []");
        let error = parse_recipe(text.as_bytes(), "local").unwrap_err();
        assert!(error.contains("not declared"), "{error}");
    }

    #[test]
    fn rejects_forward_reference() {
        let text = VALID.replace("{{value.value}}", "{{text.value}}");
        let error = parse_recipe(text.as_bytes(), "local").unwrap_err();
        assert!(error.contains("earlier step"), "{error}");
    }

    #[test]
    fn a_write_budget_without_write_steps_is_harmless() {
        let text = VALID.replace("writes = 0", "writes = 2");
        parse_recipe(text.as_bytes(), "local").unwrap();
    }

    #[test]
    fn built_in_digest_is_valid() {
        let recipe = audit_digest();
        validate_recipe(&recipe).unwrap();
        assert_eq!(recipe.source, "builtin");
        assert!(!recipe.has_catalogue_reads());
    }

    #[test]
    fn workspace_prep_projects_and_bounds_inputs() {
        let recipes = builtins();
        let recipe = recipes
            .iter()
            .find(|recipe| recipe.id == "workspace.prep")
            .unwrap();
        assert_eq!(recipe.steps.len(), 4);
        let schema = recipe.steps[0].schema.as_ref().unwrap();
        assert_eq!(schema["properties"]["workspace"]["minimum"], 1);
        assert_eq!(schema["properties"]["workspace"]["maximum"], 10);
        assert_eq!(schema["properties"]["program"]["maxLength"], 128);
        assert_eq!(schema["properties"]["dnd"]["type"], "boolean");
        assert_eq!(recipe.budget.writes, 3);
        assert_eq!(
            recipe.steps[1].args.as_ref().unwrap()["workspace_number"],
            "{{choose.workspace}}"
        );
        assert_eq!(
            recipe.steps[2].args.as_ref().unwrap()["args"],
            serde_json::json!([])
        );
    }

    #[test]
    fn update_assistant_projects_an_each_step() {
        let recipes = builtins();
        let recipe = recipes
            .iter()
            .find(|recipe| recipe.id == "updates.install")
            .unwrap();
        assert_eq!(recipe.steps.len(), 2);
        assert!(matches!(recipe.steps[0].kind, StepKind::Read));
        assert!(matches!(recipe.steps[1].kind, StepKind::Each));
        let each = &recipe.steps[1];
        assert_eq!(each.from.as_deref(), Some("plan.updates"));
        assert_eq!(each.operation.as_deref(), Some("packages.install_pinned"));
        assert_eq!(
            each.args.as_ref().unwrap()["publisher"],
            "{{item.publisher}}"
        );
        assert_eq!(
            each.args.as_ref().unwrap()["version"],
            "{{item.available_version}}"
        );
        assert_eq!(recipe.budget.writes, 32);
        assert_eq!(recipe.budget.seconds, 180);
    }

    #[test]
    fn builtins_are_valid_and_include_write_recipes() {
        let recipes = builtins();
        for recipe in &recipes {
            validate_recipe(recipe).unwrap();
        }
        let desktop = recipes
            .iter()
            .find(|recipe| recipe.id == "desktop.check")
            .unwrap();
        assert_eq!(desktop.steps.len(), 5);
        assert!(matches!(desktop.steps[0].kind, StepKind::Read));
        assert!(matches!(desktop.steps[4].kind, StepKind::Render));
        assert!(
            desktop.steps[4]
                .template
                .as_deref()
                .is_some_and(|template| template.contains("{{notes.count}}"))
        );
        let updates = recipes
            .iter()
            .find(|recipe| recipe.id == "updates.watch")
            .unwrap();
        assert_eq!(updates.steps.len(), 2);
        let ai = recipes
            .iter()
            .find(|recipe| recipe.id == "ai.check")
            .unwrap();
        assert_eq!(ai.steps.len(), 5);
        assert!(matches!(ai.steps[4].kind, StepKind::Render));
        let workspace = recipes
            .iter()
            .find(|recipe| recipe.id == "workspace.compose")
            .unwrap();
        assert_eq!(workspace.write_count(), 3);
        assert_eq!(workspace.budget.writes, 3);
        let settings = recipes
            .iter()
            .find(|recipe| recipe.id == "settings.concierge")
            .unwrap();
        assert_eq!(settings.write_count(), 1);
        let dnd = recipes
            .iter()
            .find(|recipe| recipe.id == "notifications.dnd")
            .unwrap();
        assert_eq!(dnd.write_count(), 1);
        let doctor = recipes
            .iter()
            .find(|recipe| recipe.id == "packages.doctor")
            .unwrap();
        assert_eq!(doctor.write_count(), 0);
        assert!(
            doctor
                .operations
                .iter()
                .any(|operation| operation == "packages.list_installed")
        );
        let updates = recipes
            .iter()
            .find(|recipe| recipe.id == "packages.updates")
            .unwrap();
        assert_eq!(updates.write_count(), 1);
        assert!(
            updates
                .operations
                .iter()
                .any(|operation| operation == "packages.install_pinned")
        );
    }

    #[test]
    fn each_must_be_final_and_reference_an_earlier_step() {
        let text = r#"
id = "test.each"
title = "Each"
operations = ["packages.update_plan", "packages.install_pinned"]
budget = { steps = 3, writes = 4, seconds = 10 }

[[step]]
id = "plan"
kind = "read"
operation = "packages.update_plan"

[[step]]
id = "apply"
kind = "each"
from = "plan.updates"
operation = "packages.install_pinned"
args = { publisher = "{{item.publisher}}" }

[[step]]
id = "text"
kind = "render"
template = "done"
"#;
        let error = parse_recipe(text.as_bytes(), "local").unwrap_err();
        assert!(
            error.contains("final steps") || error.contains("final step"),
            "{error}"
        );

        let text = r#"
id = "test.each"
title = "Each"
operations = ["packages.update_plan", "packages.install_pinned"]
budget = { steps = 2, writes = 4, seconds = 10 }

[[step]]
id = "plan"
kind = "read"
operation = "packages.update_plan"

[[step]]
id = "apply"
kind = "each"
from = "later.updates"
operation = "packages.install_pinned"
args = { publisher = "{{item.publisher}}" }
"#;
        let error = parse_recipe(text.as_bytes(), "local").unwrap_err();
        assert!(error.contains("earlier step"), "{error}");
    }

    #[test]
    fn inputs_schema_merges_select_slots() {
        let text = r#"
id = "test.inputs"
title = "Inputs"
budget = { steps = 2, writes = 0, seconds = 10 }

[[step]]
id = "choose"
kind = "select"
schema = { type = "object", properties = { workspace = { type = "integer", minimum = 1, maximum = 9 }, focus = { type = "boolean" } }, required = ["workspace"] }

[[step]]
id = "render"
kind = "render"
template = "ws {{choose.workspace}}"
"#;
        let recipe = parse_recipe(text.as_bytes(), "local").unwrap();
        let schema = recipe.inputs_schema();
        assert_eq!(schema["properties"]["workspace"]["maximum"], 9);
        assert_eq!(schema["required"][0], "workspace");
    }
}
