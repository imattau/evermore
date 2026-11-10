//! Projects a `.ns` recipe (see [`crate` root docs and
//! `crates/evermore-agentd/nscript/README.md`]) into the existing
//! [`crate::recipe::Recipe`]/[`crate::recipe::RecipeStep`] structs, which
//! [`crate::recipe::validate_recipe`] then validates exactly as it validates
//! a hand-written TOML recipe. `nscript`'s own runtime, `OperationHost`, and
//! simulator never run here: this crate only uses `nscript`'s parser and
//! checker as a typed front end, the same way a compiler uses a parser
//! library without running the program it parses.
//!
//! # What this understands
//!
//! A recipe `.ns` file is expected to declare, at the top level:
//! - zero or more `input name: Type` declarations, projected into one
//!   `select` step (id `choose`);
//! - exactly one `every`/`at` schedule body, whose own top level contains,
//!   in order:
//!   - zero or more `let name = module.operation(args)?` statements, each a
//!     catalogue operation call, projected into one `read` or `write` step
//!     per statement (its step id is the `let` name; whether it becomes a
//!     `read` or `write` step comes from looking the real catalogue
//!     operation up in `evermore_common::operations`, never from the
//!     script);
//!   - at most one `print(...)` statement, built from string-literal and
//!     `+`-concatenated member-access/`text(...)` parts, projected into one
//!     `render` step (id `report`) whose template uses the same
//!     `{{step.field}}` placeholder syntax a hand-written TOML recipe does.
//!     The `render` step is projected at its own position in the schedule
//!     body, not always last: a `print(...)` before a later write previews
//!     already-known values ahead of it, which
//!     [`crate::recipe::validate_recipe`]'s "writes must be final" rule
//!     allows; a `print(...)` after a write is still refused by that same
//!     rule.
//! - optionally, two leading line comments naming the recipe's title and
//!   description (`nscript` has no such metadata of its own):
//!   ```text
//!   // title: Package doctor
//!   // description: Inspect installed packages ...
//!   ```
//!   A recipe's `id` is always its file name without the `.ns` extension.
//!
//! Anything else in the schedule body (an `if`, a `for`, a nested call, a
//! second `print`) is refused with a specific message rather than silently
//! dropped or guessed at.
//!
//! # Module names
//!
//! `use evermore_<prefix>` resolves, inside this projector, to the real
//! catalogue operation prefix `<prefix>` (`evermore_packages` ->
//! `packages.*`). The modules themselves are Evermore's own, non-NIP
//! `nscript` module files, bundled into this binary at compile time (see
//! [`evermore_module_registry`]) so the daemon never reads them from disk at
//! run time.

use std::collections::{BTreeMap, BTreeSet};

use evermore_common::agent::{StepKind, TaskBudget};
use evermore_common::operations::{self, Access};
use nscript_modules::{ModuleDependency, ModuleOrigin, ModuleRegistry, parse_module};
use nscript_syntax::ast::{Expr, ExprKind, Item, StatementKind};
use nscript_syntax::{Diagnostic, Program, parse_program};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::recipe::{Recipe, RecipeStep, validate_recipe};

const EVERMORE_PACKAGES_NSM: &str = include_str!("../nscript/modules/evermore_packages/0.1.0.nsm");
const EVERMORE_NOTIFICATIONS_NSM: &str =
    include_str!("../nscript/modules/evermore_notifications/0.1.0.nsm");
const EVERMORE_AI_NSM: &str = include_str!("../nscript/modules/evermore_ai/0.1.0.nsm");
const EVERMORE_SWAY_NSM: &str = include_str!("../nscript/modules/evermore_sway/0.1.0.nsm");

/// Every Evermore-specific, non-NIP `nscript` module this crate bundles, as
/// `(module name, source text)`. The single list both
/// [`evermore_module_registry`] registers from and
/// `tests::bundled_module_signatures_match_the_real_catalogue` cross-checks
/// against `evermore_common::operations::catalog()` — adding a module here
/// is enough to get both.
const BUNDLED_MODULES: &[(&str, &str)] = &[
    ("evermore_packages", EVERMORE_PACKAGES_NSM),
    ("evermore_notifications", EVERMORE_NOTIFICATIONS_NSM),
    ("evermore_ai", EVERMORE_AI_NSM),
    ("evermore_sway", EVERMORE_SWAY_NSM),
];

/// A registry carrying `nscript`'s standard modules plus Evermore's own,
/// bundled from source at compile time rather than read from disk.
///
/// # Panics
///
/// Panics if a bundled `.nsm` file is invalid; a test below catches this in
/// CI before it could reach a built daemon.
fn evermore_module_registry() -> ModuleRegistry {
    let mut registry = ModuleRegistry::with_builtins();
    for &(name, source) in BUNDLED_MODULES {
        let (descriptor, diagnostics) = parse_module(source);
        assert!(
            diagnostics.is_empty(),
            "invalid bundled module {name}: {diagnostics:?}"
        );
        registry
            .register(
                descriptor.expect("a valid module has a descriptor"),
                ModuleOrigin::BuiltIn(name.to_owned()),
            )
            .expect("bundled modules have unique identities");
    }
    registry
}

/// The real catalogue operation prefix an `nscript` module name stands for.
/// Most modules follow the single-level `evermore_<prefix>` convention
/// (`evermore_packages` -> `packages.*`).
fn catalogue_prefix(module: &str) -> Option<&str> {
    module.strip_prefix("evermore_")
}

/// The exact catalogue operation a `(module, operation)` pair stands for.
/// `evermore_ai` is special because the catalogue nests it
/// (`ai.infer.status`), so its short nscript names map explicitly.
fn catalogue_operation(module: &str, operation: &str) -> Option<String> {
    match (module, operation) {
        ("evermore_ai", "infer_status") => Some("ai.infer.status".to_string()),
        ("evermore_ai", "model_list") => Some("ai.model.list".to_string()),
        ("evermore_ai", "index_status") => Some("ai.index.status".to_string()),
        ("evermore_ai", "context_status") => Some("ai.context.status".to_string()),
        _ => catalogue_prefix(module).map(|prefix| format!("{prefix}.{operation}")),
    }
}

fn describe_diagnostics(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The recipe's title and description from its two leading `// title: ...`
/// / `// description: ...` comment lines, in either order, read directly
/// from the source text (`nscript` discards comments, and has no metadata
/// fields of its own for either).
fn extract_metadata(text: &str) -> Result<(String, String), String> {
    let mut title = None;
    let mut description = None;
    for line in text.lines() {
        let Some(comment) = line.trim_start().strip_prefix("//") else {
            break;
        };
        let comment = comment.trim_start();
        if let Some(value) = comment.strip_prefix("title:") {
            title = Some(value.trim().to_owned());
        } else if let Some(value) = comment.strip_prefix("description:") {
            description = Some(value.trim().to_owned());
        }
    }
    let title = title.ok_or("recipe needs a leading `// title: ...` comment")?;
    Ok((title, description.unwrap_or_default()))
}

fn json_schema_type(type_name: &str) -> Result<&'static str, String> {
    match type_name {
        "Text" | "PubKey" => Ok("string"),
        "Int" | "Integer" => Ok("integer"),
        "Bool" => Ok("boolean"),
        other => Err(format!(
            "input declares type `{other}`, which this projector cannot turn into a select \
             step's JSON schema (only Text, Int, Bool and PubKey can)"
        )),
    }
}

/// Module parameters can be lists (for example `sway.launch_app(args)`), even
/// though recipe `input` declarations cannot; keep the two mappings separate.
#[cfg(test)]
fn module_param_json_type(type_name: &str) -> Result<&'static str, String> {
    match type_name {
        "List<Text>" => Ok("array"),
        other => json_schema_type(other),
    }
}

/// `module.operation(args)?`'s pieces, or `None` for any other shape of
/// `let` value. The trailing `?` ([`ExprKind::Propagate`]) is required: a
/// catalogue operation always declares `Result<T,E>`.
fn as_operation_call(value: &Expr) -> Option<(&str, &str, &[Expr])> {
    let ExprKind::Propagate(inner) = &value.value else {
        return None;
    };
    let ExprKind::Call { callee, arguments } = &inner.value else {
        return None;
    };
    let ExprKind::Member { value, name } = &callee.value else {
        return None;
    };
    let ExprKind::Identifier(module) = &value.value else {
        return None;
    };
    Some((module.as_str(), name.value.as_str(), arguments.as_slice()))
}

/// Copy the consuming catalogue operation's bounds onto a declared input's
/// select schema. `input workspace: Int` has no range of its own, but
/// `sway.switch_workspace` accepts 1..10; without this, the panel form would
/// default to an invalid 0 and the run would fail.
fn collect_input_constraints(
    constraints: &mut BTreeMap<String, Map<String, Value>>,
    argument: &Expr,
    inputs: &BTreeSet<String>,
    definition: &operations::Definition,
    parameter: &str,
) {
    let ExprKind::Identifier(name) = &argument.value else {
        return;
    };
    if !inputs.contains(name) {
        return;
    }
    let Some(property) = definition
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(parameter))
        .and_then(Value::as_object)
    else {
        return;
    };
    let mut extra = Map::new();
    for key in [
        "minimum",
        "maximum",
        "minLength",
        "maxLength",
        "pattern",
        "enum",
    ] {
        if let Some(value) = property.get(key) {
            extra.insert(key.to_string(), value.clone());
        }
    }
    if !extra.is_empty() {
        constraints.entry(name.clone()).or_default().extend(extra);
    }
}

/// One operation-call argument, as a `RecipeStep::args` JSON value: a
/// literal passes through as itself; a bare `input` reference becomes a
/// `{{choose.<name>}}` placeholder the runner substitutes at execution time;
/// and inside a `for <binding> in ...` loop, a `<binding>.<field>` reference
/// becomes `{{item.<field>}}`, matching the runner's `each` step.
fn arg_value(
    expr: &Expr,
    inputs: &BTreeSet<String>,
    loop_binding: Option<&str>,
) -> Result<Value, String> {
    match &expr.value {
        ExprKind::Text(text) => Ok(Value::String(text.clone())),
        ExprKind::Integer(value) => Ok(json!(value)),
        ExprKind::Bool(value) => Ok(Value::Bool(*value)),
        ExprKind::List(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(arg_value(item, inputs, loop_binding)?);
            }
            Ok(Value::Array(values))
        }
        ExprKind::Identifier(name) if inputs.contains(name) => {
            Ok(Value::String(format!("{{{{choose.{name}}}}}")))
        }
        ExprKind::Identifier(name) => Err(format!(
            "operation argument `{name}` is not a declared input, and this projector does not \
             resolve any other kind of value reference as an argument"
        )),
        ExprKind::Member { .. } => {
            let Some(binding) = loop_binding else {
                return Err(
                    "operation arguments may reference a loop item only inside a `for` body".into(),
                );
            };
            let path = expression_path(expr).ok_or_else(|| {
                "a loop-item argument must be a plain field path such as `update.name`".to_string()
            })?;
            let suffix = path.strip_prefix(&format!("{binding}.")).ok_or_else(|| {
                format!("loop-item argument `{path}` must start with `{binding}.`")
            })?;
            Ok(Value::String(format!("{{{{item.{suffix}}}}}")))
        }
        _ => Err(
            "operation arguments must be a literal, a bare input name, or a loop-item field".into(),
        ),
    }
}

/// The dotted path an expression names (`plan.updates`, `update.publisher`,
/// or a bare identifier). Used for `for` sources and loop-item arguments.
fn expression_path(expr: &Expr) -> Option<String> {
    match &expr.value {
        ExprKind::Identifier(name) => Some(name.clone()),
        ExprKind::Member { value, name } => {
            Some(format!("{}.{}", expression_path(value)?, name.value))
        }
        _ => None,
    }
}

/// One concatenated piece of a `print(...)` argument: literal text, or a
/// `{{step.field}}` placeholder.
enum TemplatePart {
    Literal(String),
    Placeholder(String),
}

/// The dotted path a `print` argument's sub-expression refers to (e.g.
/// `installed.count`, or a bare `publisher`), for turning into a
/// `{{...}}` placeholder. Bare identifiers that name a declared input
/// resolve through the `choose` step, exactly as [`arg_value`] resolves one
/// as an operation argument.
fn placeholder_path(expr: &Expr, inputs: &BTreeSet<String>) -> Result<String, String> {
    match &expr.value {
        ExprKind::Identifier(name) if inputs.contains(name) => Ok(format!("choose.{name}")),
        ExprKind::Identifier(name) => Ok(name.clone()),
        ExprKind::Member { value, name } => Ok(format!(
            "{}.{}",
            placeholder_path(value, inputs)?,
            name.value
        )),
        _ => Err(
            "print's argument mixes text with something other than a plain field \
                  reference or `text(...)` of one"
                .into(),
        ),
    }
}

fn flatten_template(
    expr: &Expr,
    inputs: &BTreeSet<String>,
    parts: &mut Vec<TemplatePart>,
) -> Result<(), String> {
    match &expr.value {
        ExprKind::Binary {
            operator,
            left,
            right,
        } if operator == "+" => {
            flatten_template(left, inputs, parts)?;
            flatten_template(right, inputs, parts)?;
            Ok(())
        }
        ExprKind::Text(text) => {
            parts.push(TemplatePart::Literal(text.clone()));
            Ok(())
        }
        ExprKind::Call { callee, arguments } if matches!(&callee.value, ExprKind::Identifier(name) if name == "text") =>
        {
            let [argument] = arguments.as_slice() else {
                return Err("`text(...)` takes exactly one argument in a print template".into());
            };
            parts.push(TemplatePart::Placeholder(placeholder_path(
                argument, inputs,
            )?));
            Ok(())
        }
        ExprKind::Identifier(_) | ExprKind::Member { .. } => {
            parts.push(TemplatePart::Placeholder(placeholder_path(expr, inputs)?));
            Ok(())
        }
        _ => Err(
            "print's argument must be string literals, field references, or `text(...)` \
                  of one, joined with `+`"
                .into(),
        ),
    }
}

fn render_template(expr: &Expr, inputs: &BTreeSet<String>) -> Result<String, String> {
    let mut parts = Vec::new();
    flatten_template(expr, inputs, &mut parts)?;
    let mut template = String::new();
    for part in parts {
        match part {
            TemplatePart::Literal(text) => template.push_str(&text),
            TemplatePart::Placeholder(path) => {
                template.push_str("{{");
                template.push_str(&path);
                template.push_str("}}");
            }
        }
    }
    Ok(template)
}

fn is_print_call(expr: &Expr) -> bool {
    matches!(
        &expr.value,
        ExprKind::Call { callee, .. } if matches!(&callee.value, ExprKind::Identifier(name) if name == "print")
    )
}

/// Parses, checks, and projects `bytes` (a `.ns` file whose id is
/// `recipe_id`, typically its file name without the extension) into a
/// [`Recipe`], then validates it exactly as [`crate::recipe::parse_recipe`]
/// validates a hand-written TOML recipe.
///
/// # Errors
///
/// Returns a specific message for a parse, module-resolution, or type
/// error; for any program shape this projector does not understand (see
/// the module doc); or for a projected recipe that fails
/// [`validate_recipe`].
pub fn parse_ns_recipe(bytes: &[u8], source: &str, recipe_id: &str) -> Result<Recipe, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|error| format!("recipe is not UTF-8: {error}"))?;
    let (title, description) = extract_metadata(text)?;

    let (program, mut diagnostics): (Program, Vec<Diagnostic>) = parse_program(text);
    if !diagnostics.is_empty() {
        return Err(format!(
            "nscript parse error: {}",
            describe_diagnostics(&diagnostics)
        ));
    }

    let registry = evermore_module_registry();
    let mut roots = Vec::new();
    for import in &program.imports {
        let requirement_text = import.requirement.as_deref().unwrap_or("*");
        let requirement = semver::VersionReq::parse(requirement_text)
            .map_err(|error| format!("invalid module requirement `{requirement_text}`: {error}"))?;
        roots.push(ModuleDependency {
            name: import.path.clone(),
            requirement,
        });
    }
    let graph = registry
        .resolve(&roots)
        .map_err(|error| format!("module resolution failed: {error}"))?;

    diagnostics.extend(nscript_semantics::analyze_with_modules(&program, &graph));
    if !diagnostics.is_empty() {
        return Err(format!(
            "nscript check error: {}",
            describe_diagnostics(&diagnostics)
        ));
    }
    let (checked, check_diagnostics) = nscript_semantics::check(&program);
    if !check_diagnostics.is_empty() {
        return Err(format!(
            "nscript check error: {}",
            describe_diagnostics(&check_diagnostics)
        ));
    }
    let checked = checked.expect("empty diagnostics means a checked program");

    let input_names: BTreeSet<String> = checked.inputs.keys().cloned().collect();

    let mut steps = Vec::new();
    if !checked.inputs.is_empty() {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for (name, type_name) in &checked.inputs {
            properties.insert(
                name.clone(),
                json!({ "type": json_schema_type(type_name)? }),
            );
            required.push(Value::String(name.clone()));
        }
        steps.push(RecipeStep {
            id: "choose".to_owned(),
            kind: StepKind::Select,
            operation: None,
            args: None,
            schema: Some(json!({
                "type": "object",
                "properties": Value::Object(properties),
                "required": required,
            })),
            template: None,
            view: None,
            limit: None,
            from: None,
        });
    }

    let mut schedule_body = None;
    for item in &program.ast.items {
        if let Item::Statement(statement) = item
            && let StatementKind::Every { body, .. } | StatementKind::At { body, .. } =
                &statement.value
        {
            if schedule_body.is_some() {
                return Err("a recipe may declare only one every/at schedule".into());
            }
            schedule_body = Some(body);
        }
    }
    let body =
        schedule_body.ok_or("recipe needs exactly one every/at block containing its steps")?;

    let mut operations = Vec::new();
    let mut seen_render = false;
    let mut seen_for = false;
    let mut input_constraints: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for item in body {
        match item {
            Item::Let(declaration) => {
                let (module, operation, arguments) = as_operation_call(&declaration.value).ok_or(
                    "every `let` in a recipe's schedule body must call one catalogue \
                            operation, ending in `?`",
                )?;
                let real_name = catalogue_operation(module, operation).ok_or_else(|| {
                    format!(
                        "module `{module}` does not follow the `evermore_<prefix>` naming \
                              convention this projector requires"
                    )
                })?;
                let definition = operations::find(&real_name)
                    .ok_or_else(|| format!("`{real_name}` is not a known catalogue operation"))?;
                let kind = match definition.access {
                    Access::Read => StepKind::Read,
                    Access::Write => StepKind::Write,
                };
                let args = if arguments.is_empty() {
                    None
                } else {
                    // Named by the *module's own* declared parameter order
                    // (`nscript` arguments are positional), never by the
                    // catalogue's `input_schema` key order: a JSON object's
                    // key order is not guaranteed to match either, and
                    // nscript's checker is what actually enforced this call
                    // matches the module's declared signature.
                    let parameters = &graph
                        .modules
                        .get(module)
                        .and_then(|registered| {
                            registered
                                .descriptor
                                .operations
                                .iter()
                                .find(|op| op.name == operation)
                        })
                        .ok_or_else(|| {
                            format!(
                                "module `{module}` has no resolved descriptor for `{operation}`"
                            )
                        })?
                        .parameters;
                    if parameters.len() != arguments.len() {
                        return Err(format!(
                            "`{module}.{operation}` takes {} argument(s), found {}",
                            parameters.len(),
                            arguments.len()
                        ));
                    }
                    let mut object = Map::new();
                    for (parameter, argument) in parameters.iter().zip(arguments) {
                        collect_input_constraints(
                            &mut input_constraints,
                            argument,
                            &input_names,
                            &definition,
                            &parameter.name,
                        );
                        object.insert(
                            parameter.name.clone(),
                            arg_value(argument, &input_names, None)?,
                        );
                    }
                    Some(Value::Object(object))
                };
                operations.push(real_name.clone());
                steps.push(RecipeStep {
                    id: declaration.name.value.clone(),
                    kind,
                    operation: Some(real_name),
                    args,
                    schema: None,
                    template: None,
                    view: None,
                    limit: None,
                    from: None,
                });
            }
            Item::Statement(statement) => match &statement.value {
                StatementKind::Expression(expr) => {
                    if !is_print_call(expr) {
                        return Err(
                            "a recipe's schedule body supports only `let <op>?` statements, one \
                             `print(...)`, and one `for` write loop, not this expression"
                                .into(),
                        );
                    }
                    if seen_render {
                        return Err("a recipe may have only one `print(...)` statement".into());
                    }
                    seen_render = true;
                    let ExprKind::Call { arguments, .. } = &expr.value else {
                        unreachable!("is_print_call checked this");
                    };
                    let [argument] = arguments.as_slice() else {
                        return Err("`print(...)` in a recipe takes exactly one argument".into());
                    };
                    let template = render_template(argument, &input_names)?;
                    // Pushed at its own position in the schedule body, not
                    // deferred to the end: a `print(...)` written before a
                    // later write call previews already-known values ahead of
                    // it, which `validate_recipe`'s "writes must be final"
                    // rule allows (see nscript/README.md). A `print(...)`
                    // written after a write is still refused by that same
                    // rule, exactly as a hand-written TOML recipe's `render`
                    // step would be.
                    steps.push(RecipeStep {
                        id: "report".to_owned(),
                        kind: StepKind::Render,
                        operation: None,
                        args: None,
                        schema: None,
                        template: Some(template),
                        view: None,
                        limit: None,
                        from: None,
                    });
                }
                StatementKind::For {
                    binding,
                    value,
                    body,
                } => {
                    if seen_for {
                        return Err(
                            "a recipe may declare only one `for` write loop; use one `each` step"
                                .into(),
                        );
                    }
                    seen_for = true;
                    let source_path = expression_path(value).ok_or_else(|| {
                        "a for loop's source must be a field path such as `plan.updates`"
                            .to_string()
                    })?;
                    let base = source_path.split('.').next().unwrap_or("");
                    if !steps
                        .iter()
                        .any(|step| step.id == base && matches!(step.kind, StepKind::Read))
                    {
                        return Err(format!(
                            "for loop source `{source_path}` must start with an earlier read step"
                        ));
                    }
                    let [Item::Let(declaration)] = body.as_slice() else {
                        return Err(
                            "a for body must contain exactly one `let <write operation>?` statement"
                                .into(),
                        );
                    };
                    let (module, operation, arguments) = as_operation_call(&declaration.value)
                        .ok_or(
                            "a for body's `let` must call one catalogue write operation, ending \
                             in `?`",
                        )?;
                    let real_name = catalogue_operation(module, operation).ok_or_else(|| {
                        format!(
                            "module `{module}` does not follow the `evermore_<prefix>` naming \
                             convention this projector requires"
                        )
                    })?;
                    let definition = operations::find(&real_name).ok_or_else(|| {
                        format!("`{real_name}` is not a known catalogue operation")
                    })?;
                    if definition.access != Access::Write {
                        return Err(format!(
                            "for loop operation `{real_name}` must be an approval-gated write"
                        ));
                    }
                    let parameters = &graph
                        .modules
                        .get(module)
                        .and_then(|registered| {
                            registered
                                .descriptor
                                .operations
                                .iter()
                                .find(|op| op.name == operation)
                        })
                        .ok_or_else(|| {
                            format!(
                                "module `{module}` has no resolved descriptor for `{operation}`"
                            )
                        })?
                        .parameters;
                    if parameters.len() != arguments.len() {
                        return Err(format!(
                            "`{module}.{operation}` takes {} argument(s), found {}",
                            parameters.len(),
                            arguments.len()
                        ));
                    }
                    let mut object = Map::new();
                    for (parameter, argument) in parameters.iter().zip(arguments) {
                        collect_input_constraints(
                            &mut input_constraints,
                            argument,
                            &input_names,
                            &definition,
                            &parameter.name,
                        );
                        object.insert(
                            parameter.name.clone(),
                            arg_value(argument, &input_names, Some(&binding.value))?,
                        );
                    }
                    operations.push(real_name.clone());
                    steps.push(RecipeStep {
                        id: declaration.name.value.clone(),
                        kind: StepKind::Each,
                        operation: Some(real_name),
                        args: Some(Value::Object(object)),
                        schema: None,
                        template: None,
                        view: None,
                        limit: None,
                        from: Some(source_path),
                    });
                }
                _ => {
                    return Err(
                        "a recipe's schedule body supports only `let <op>?` statements, one \
                         `print(...)`, and one `for` write loop, not this declaration"
                            .into(),
                    );
                }
            },
            _ => {
                return Err(
                    "a recipe's schedule body supports only `let <op>?` statements, one \
                     `print(...)`, and one `for` write loop, not this declaration"
                        .into(),
                );
            }
        }
    }

    if !input_constraints.is_empty() {
        if let Some(select) = steps.iter_mut().find(|step| step.kind == StepKind::Select)
            && let Some(properties) = select
                .schema
                .as_mut()
                .and_then(|schema| schema.get_mut("properties"))
                .and_then(Value::as_object_mut)
        {
            for (name, extra) in input_constraints {
                if let Some(property) = properties.get_mut(&name).and_then(Value::as_object_mut) {
                    for (key, value) in extra {
                        property.insert(key, value);
                    }
                }
            }
        }
    }

    let has_each = steps.iter().any(|step| matches!(step.kind, StepKind::Each));
    let writes = if has_each {
        32
    } else {
        steps
            .iter()
            .filter(|step| matches!(step.kind, StepKind::Write | StepKind::Each))
            .count() as u32
    };
    let recipe = Recipe {
        id: recipe_id.to_owned(),
        version: 1,
        title,
        description,
        operations,
        budget: TaskBudget {
            steps: steps.len() as u32,
            writes,
            seconds: if has_each { 180 } else { 45 },
            context_tokens: 0,
        },
        steps,
        source: source.to_owned(),
        sha256: hex::encode(Sha256::digest(bytes)),
    };
    validate_recipe(&recipe)?;
    Ok(recipe)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGES_DOCTOR_NS: &str = include_str!("../nscript/recipes/packages.doctor.ns");
    const NOTIFICATIONS_TRIAGE_NS: &str =
        include_str!("../nscript/recipes/notifications.triage.ns");

    #[test]
    fn bundled_modules_are_valid() {
        // Catches a broken bundled `.nsm` file at test time instead of at
        // the first `parse_ns_recipe` call a running daemon makes.
        let _ = evermore_module_registry();
    }

    #[test]
    fn packages_doctor_ns_projects_onto_the_hand_written_toml_recipe() {
        let recipe = parse_ns_recipe(PACKAGES_DOCTOR_NS.as_bytes(), "builtin", "packages.doctor")
            .expect("projects and validates");
        assert_eq!(recipe.id, "packages.doctor");
        assert_eq!(recipe.title, "Package doctor");
        assert_eq!(
            recipe.description,
            "Inspect installed packages and explain one package's direct dependencies. \
             Read-only."
        );
        assert_eq!(
            recipe.operations,
            ["packages.list_installed", "packages.explain_dependencies"]
        );
        assert_eq!(recipe.budget.steps, 4);
        assert_eq!(recipe.budget.writes, 0);

        let ids: Vec<&str> = recipe.steps.iter().map(|step| step.id.as_str()).collect();
        assert_eq!(ids, ["choose", "installed", "dependencies", "report"]);

        assert_eq!(recipe.steps[0].kind, StepKind::Select);
        let schema = recipe.steps[0]
            .schema
            .as_ref()
            .expect("select has a schema");
        assert_eq!(
            schema["properties"]["publisher"]["type"],
            Value::String("string".to_owned())
        );
        assert_eq!(
            schema["required"],
            Value::Array(vec![
                Value::String("name".to_owned()),
                Value::String("publisher".to_owned())
            ])
        );

        assert_eq!(recipe.steps[1].kind, StepKind::Read);
        assert_eq!(
            recipe.steps[1].operation.as_deref(),
            Some("packages.list_installed")
        );
        assert_eq!(recipe.steps[1].args, None);

        assert_eq!(recipe.steps[2].kind, StepKind::Read);
        assert_eq!(
            recipe.steps[2].operation.as_deref(),
            Some("packages.explain_dependencies")
        );
        assert_eq!(
            recipe.steps[2].args,
            Some(json!({
                "publisher": "{{choose.publisher}}",
                "name": "{{choose.name}}",
            }))
        );

        assert_eq!(recipe.steps[3].kind, StepKind::Render);
        assert_eq!(
            recipe.steps[3].template.as_deref(),
            Some(
                "Installed packages: {{installed.count}}\nSelected: {{choose.publisher}}/\
                 {{choose.name}} version {{dependencies.version}}"
            )
        );
    }

    #[test]
    fn notifications_triage_ns_projects_a_read_then_a_render_then_a_write_step() {
        // M12.4: a genuinely new recipe with no hand-written TOML
        // counterpart, exercising the projector's write-step path for the
        // first time (packages.doctor only ever produced read + render),
        // plus a render step placed at its own source position (previewing
        // the write) rather than always forced to the end.
        let recipe = parse_ns_recipe(
            NOTIFICATIONS_TRIAGE_NS.as_bytes(),
            "builtin",
            "notifications.triage",
        )
        .expect("projects and validates");
        assert_eq!(recipe.id, "notifications.triage");
        assert_eq!(recipe.title, "Notification triage");
        assert_eq!(
            recipe.operations,
            ["notifications.list_recent", "notifications.dismiss"]
        );
        assert_eq!(recipe.budget.steps, 4);
        assert_eq!(recipe.budget.writes, 1);

        let ids: Vec<&str> = recipe.steps.iter().map(|step| step.id.as_str()).collect();
        assert_eq!(ids, ["choose", "recent", "report", "result"]);

        assert_eq!(recipe.steps[0].kind, StepKind::Select);
        assert_eq!(
            recipe.steps[0]
                .schema
                .as_ref()
                .expect("select has a schema")["required"],
            Value::Array(vec![Value::String("notification_id".to_owned())])
        );

        assert_eq!(recipe.steps[1].kind, StepKind::Read);
        assert_eq!(
            recipe.steps[1].operation.as_deref(),
            Some("notifications.list_recent")
        );
        assert_eq!(recipe.steps[1].args, None);

        assert_eq!(recipe.steps[2].kind, StepKind::Render);
        assert_eq!(
            recipe.steps[2].template.as_deref(),
            Some("About to dismiss notification {{choose.notification_id}}")
        );

        assert_eq!(recipe.steps[3].kind, StepKind::Write);
        assert_eq!(
            recipe.steps[3].operation.as_deref(),
            Some("notifications.dismiss")
        );
        assert_eq!(
            recipe.steps[3].args,
            Some(json!({ "notification_id": "{{choose.notification_id}}" }))
        );
    }

    #[test]
    fn bundled_module_signatures_match_the_real_catalogue() {
        // Guards against the drift M12.1 flagged as a risk: each bundled
        // `.nsm` module's own operation signatures are a second, hand-
        // maintained copy of evermore_common::operations::catalog()'s real
        // ones. Nothing else cross-checks parameter names, counts, or
        // types between the two -- nscript's own checker only validates a
        // call against the module's *own* declared signature, never
        // against the real catalogue.
        for &(module_name, source) in BUNDLED_MODULES {
            let (descriptor, diagnostics) = parse_module(source);
            assert!(diagnostics.is_empty(), "{module_name}: {diagnostics:?}");
            let descriptor = descriptor.expect("a valid module has a descriptor");
            for operation in &descriptor.operations {
                let real_name =
                    catalogue_operation(module_name, &operation.name).unwrap_or_else(|| {
                        panic!(
                            "{module_name}.{}: no catalogue operation mapping",
                            operation.name
                        )
                    });
                let definition = operations::find(&real_name).unwrap_or_else(|| {
                    panic!(
                        "{module_name}.{}: no catalogue operation `{real_name}`",
                        operation.name
                    )
                });

                let required: BTreeSet<String> = definition
                    .input_schema
                    .get("required")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect();
                let declared: BTreeSet<String> = operation
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect();
                assert_eq!(
                    declared, required,
                    "`{real_name}`: nscript declares parameters {declared:?}, the catalogue \
                     requires {required:?}"
                );

                let properties = definition.input_schema.get("properties");
                for parameter in &operation.parameters {
                    let expected =
                        module_param_json_type(&parameter.type_name).unwrap_or_else(|error| {
                            panic!("`{real_name}.{}`: {error}", parameter.name)
                        });
                    let actual = properties
                        .and_then(|properties| properties.get(&parameter.name))
                        .and_then(|property| property.get("type"))
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            panic!(
                                "`{real_name}`: catalogue property `{}` has no `type`",
                                parameter.name
                            )
                        });
                    assert_eq!(
                        actual, expected,
                        "`{real_name}.{}`: nscript declares `{}` (JSON type `{expected}`), \
                         catalogue property has JSON type `{actual}`",
                        parameter.name, parameter.type_name
                    );
                }

                // The shape of the return value matters too: a recipe that
                // prints `x.count` relies on the runtime value being the
                // array the catalogue promises, not the record the module
                // happens to declare. This caught a real drift in
                // `notifications.list_recent`/`packages.list_installed`.
                let output_is_array =
                    definition.output_schema.get("type").and_then(Value::as_str) == Some("array");
                let returns_list = operation.return_type.starts_with("Result<List<");
                assert_eq!(
                    output_is_array,
                    returns_list,
                    "`{real_name}`: catalogue output is {}, nscript returns `{}`",
                    if output_is_array {
                        "an array"
                    } else {
                        "an object"
                    },
                    operation.return_type
                );
            }
        }
    }

    #[test]
    fn a_recipe_without_a_title_comment_is_refused() {
        let source = "use evermore_packages\n\npermissions {\n    packages_read\n    clock\n}\n\nevery 1d {\n    let installed = evermore_packages.list_installed()?\n}\n";
        let error = parse_ns_recipe(source.as_bytes(), "local", "no-title").unwrap_err();
        assert!(error.contains("title"), "{error}");
    }

    #[test]
    fn an_operation_outside_the_evermore_naming_convention_is_refused() {
        let source = "// title: Bad module\nuse nip01\n\npermissions {\n    clock\n    log\n}\n\nevery 1d {\n    let note = nip01.publish_note()?\n}\n";
        let error = parse_ns_recipe(source.as_bytes(), "local", "bad-module");
        // Whatever the exact failure (unresolved module, or the naming
        // convention check), it must not silently project a recipe.
        assert!(error.is_err());
    }
}
