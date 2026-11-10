use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;

use evermore_agentd::{
    agent_runtime::AgentRuntimeStore, agents::AgentStore, clients, mcp_agent, propose, recipe,
    relay_client, render, routine::RoutineStore, runner::Runner, scheduler, store::TaskStore,
};
use evermore_common::agent::{self, AgentHealth, Request, Response, Routine, Task};
use serde_json::{Value, json};

#[derive(Clone)]
struct AgentReloadConfig {
    owner_identity: Option<String>,
    relay_url: String,
}

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("serve") => serve().map_err(|error| error.to_string()),
        Some("recipes") => recipes_cli(),
        Some("run") => run_cli(),
        Some("propose") => propose_cli(),
        Some("ask") => ask_cli(),
        Some("tasks") => tasks_cli(),
        Some("status") => status_cli(),
        Some("cancel") => cancel_cli(),
        Some("routines") => routines_cli(),
        Some("routine-add") => routine_add_cli(),
        Some("routine-enable") => routine_toggle_cli(true),
        Some("routine-disable") => routine_toggle_cli(false),
        Some("routine-remove") => routine_remove_cli(),
        Some("agents") => agents_cli(),
        Some("agents-reload") => agents_reload_cli(),
        _ => {
            eprintln!(
                "usage: evermore-agentd <serve|recipes|run|propose|ask|tasks|status|cancel|routines|routine-add|routine-enable|routine-disable|routine-remove|agents|agents-reload>"
            );
            eprintln!(
                "  serve --agent-identity NAME [--owner-identity NAME] [--owner-pubkey HEX] [--relay-url URL]"
            );
            eprintln!("  serve    run the deterministic recipe task runner");
            eprintln!("  recipes  list built-in and local recipes");
            eprintln!("  run      run a recipe: run RECIPE [JSON_INPUTS]");
            eprintln!("  propose  generate one validated write proposal: propose REQUEST");
            eprintln!("  ask      use the configured MCP agent and local tools: ask REQUEST");
            eprintln!("  tasks    list recent tasks");
            eprintln!("  status   show a task: status TASK_ID");
            eprintln!("  cancel   request task cancellation: cancel TASK_ID");
            eprintln!("  routines list standing routines");
            eprintln!(
                "  routine-add ID RECIPE --interval SECONDS --agent-identity NAME [--inputs JSON] [--disabled]"
            );
            eprintln!("  routine-enable ID | routine-disable ID | routine-remove ID");
            eprintln!("  agents   list cached relay-backed agent definitions");
            eprintln!("  agents-reload  re-read agent definitions from the relay");
            std::process::exit(2);
        }
    };

    if let Err(error) = result {
        eprintln!("evermore-agentd: {error}");
        std::process::exit(1);
    }
}

fn serve() -> std::io::Result<()> {
    let agent_identity = arg_value("--agent-identity").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "serve requires --agent-identity NAME",
        )
    })?;
    let agent_pubkey =
        clients::signer_public_key(&agent_identity).map_err(std::io::Error::other)?;
    let owner_pubkey = match arg_value("--owner-pubkey") {
        Some(pubkey) => pubkey,
        None => clients::configured_owner().map_err(std::io::Error::other)?,
    };
    let owner_identity = arg_value("--owner-identity")
        .or_else(|| std::env::var("EVERMORE_OWNER_IDENTITY").ok())
        .filter(|value| !value.is_empty());
    let relay_url = arg_value("--relay-url")
        .or_else(|| std::env::var("EVERMORE_RELAY_URL").ok())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| relay_client::DEFAULT_RELAY_URL.to_string());
    let agents = Arc::new(AgentStore::open().map_err(std::io::Error::other)?);
    let runtime = Arc::new(AgentRuntimeStore::open().map_err(std::io::Error::other)?);
    let store = Arc::new(TaskStore::open().map_err(std::io::Error::other)?);
    let routines = Arc::new(RoutineStore::open().map_err(std::io::Error::other)?);
    let runner = Arc::new(Runner {
        store: Arc::clone(&store),
        agent_identity: agent_identity.clone(),
        agent_pubkey: agent_pubkey.clone(),
        owner_pubkey: owner_pubkey.clone(),
    });
    if let Some(owner_identity) = owner_identity.clone() {
        let agents = Arc::clone(&agents);
        let runtime = Arc::clone(&runtime);
        let owner_pubkey = owner_pubkey.clone();
        let assistant_pubkey = agent_pubkey.clone();
        let relay_url = relay_url.clone();
        std::thread::spawn(move || {
            let mut last_count: Option<usize> = None;
            let mut seen_warnings = std::collections::BTreeSet::new();
            loop {
                match agents.reload(
                    &relay_url,
                    &owner_pubkey,
                    &owner_identity,
                    &assistant_pubkey,
                    &runtime,
                ) {
                    Ok(reload) => {
                        for warning in reload.warnings {
                            if seen_warnings.insert(warning.clone()) {
                                eprintln!("evermore-agentd: agent reload: {warning}");
                            }
                        }
                        if last_count != Some(reload.agents.len()) {
                            eprintln!(
                                "evermore-agentd: projected {} agent definition(s)",
                                reload.agents.len()
                            );
                            last_count = Some(reload.agents.len());
                        }
                    }
                    Err(error) => eprintln!(
                        "evermore-agentd: couldn't reload agent definitions (using the local cache): {error}"
                    ),
                }
                std::thread::sleep(std::time::Duration::from_secs(30));
            }
        });
    } else {
        eprintln!(
            "evermore-agentd: no --owner-identity given; agent definitions load from the local cache only"
        );
    }

    {
        let runner = Arc::clone(&runner);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if let Err(error) = runner.resume_waiting_tasks() {
                    eprintln!("evermore-agentd: task resume pass failed: {error}");
                }
            }
        });
    }
    spawn_routine_scheduler(Arc::clone(&runner), Arc::clone(&routines));
    scheduler::spawn(
        Arc::clone(&agents),
        Arc::clone(&runtime),
        Arc::clone(&runner),
    );

    let reload_config = AgentReloadConfig {
        owner_identity,
        relay_url,
    };

    let socket_path = agent::runtime_socket_path();
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_connection(
                    stream,
                    &runner,
                    &routines,
                    &agents,
                    &runtime,
                    &reload_config,
                ) {
                    eprintln!("evermore-agentd: request failed: {error}");
                }
            }
            Err(error) => eprintln!("evermore-agentd: accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(
    mut stream: UnixStream,
    runner: &Arc<Runner>,
    routines: &Arc<RoutineStore>,
    agents: &Arc<AgentStore>,
    runtime: &Arc<AgentRuntimeStore>,
    reload_config: &AgentReloadConfig,
) -> std::io::Result<()> {
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    if line.trim().is_empty() {
        return Ok(());
    }
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) => handle_request(request, runner, routines, agents, runtime, reload_config),
        Err(error) => Response::err(format!("invalid request: {error}")),
    };
    let mut output = serde_json::to_string(&response)
        .unwrap_or_else(|_| r#"{"status":"err","message":"serialization failed"}"#.to_string());
    output.push('\n');
    stream.write_all(output.as_bytes())?;
    stream.flush()
}

fn handle_request(
    request: Request,
    runner: &Arc<Runner>,
    routines: &Arc<RoutineStore>,
    agents: &Arc<AgentStore>,
    runtime: &Arc<AgentRuntimeStore>,
    reload_config: &AgentReloadConfig,
) -> Response {
    match request {
        Request::Health => Response::ok(AgentHealth {
            protocol_version: agent::PROTOCOL_VERSION,
            agent_pubkey: runner.agent_pubkey.clone(),
            owner_pubkey: runner.owner_pubkey.clone(),
        }),
        Request::ConfiguredAgent => Response::ok(json!({
            "agent_pubkey": runner.agent_pubkey,
            "owner_pubkey": runner.owner_pubkey,
        })),
        Request::ListAgents => {
            Response::ok(agents.views(&runner.owner_pubkey, &runner.agent_pubkey, runtime))
        }
        Request::ReloadAgents => match reload_config.owner_identity.as_deref() {
            Some(owner_identity) => match agents.reload(
                &reload_config.relay_url,
                &runner.owner_pubkey,
                owner_identity,
                &runner.agent_pubkey,
                runtime,
            ) {
                Ok(reload) => Response::ok(reload),
                Err(error) => Response::err(error),
            },
            None => {
                Response::err("agent reload requires --owner-identity NAME on the running daemon")
            }
        },
        Request::RunAgentJob { agent_id, job_id } => match agents.get(&agent_id) {
            None => Response::err(format!("unknown agent '{agent_id}'")),
            Some(agent) => match agent.jobs.iter().find(|job| job.id == job_id).cloned() {
                None => Response::err(format!("agent '{}' has no job '{job_id}'", agent.id)),
                Some(job) => match scheduler::run_job(&agent, &job, runner) {
                    Ok(task) => {
                        if let Err(error) =
                            runtime.mark_run(&agent.id, &job.id, Some(&task.task_id), None)
                        {
                            eprintln!("evermore-agentd: could not record agent run: {error}");
                        }
                        Response::ok(task)
                    }
                    Err(error) => {
                        if let Err(record_error) =
                            runtime.mark_run(&agent.id, &job.id, None, Some(error.clone()))
                        {
                            eprintln!(
                                "evermore-agentd: could not record agent error: {record_error}"
                            );
                        }
                        Response::err(error)
                    }
                },
            },
        },
        Request::ListRecipes => {
            let summaries: Vec<_> = recipe::load_all()
                .into_iter()
                .map(|recipe| recipe.summary())
                .collect();
            Response::ok(summaries)
        }
        Request::RunRecipe { recipe_id, inputs } => {
            let inputs = if inputs.is_null() { json!({}) } else { inputs };
            match recipe::find(&recipe_id) {
                Ok(recipe) => match runner.create_task(None, &recipe, inputs) {
                    Ok(task) => {
                        let task_id = task.task_id.clone();
                        let runner = Arc::clone(runner);
                        std::thread::spawn(move || runner.run(task_id, recipe));
                        Response::ok(task)
                    }
                    Err(error) => Response::err(error),
                },
                Err(error) => Response::err(error),
            }
        }
        Request::TaskStatus { task_id } => match runner.store.get(&task_id) {
            Ok(task) => Response::ok(task),
            Err(error) => Response::err(error),
        },
        Request::ListTasks { limit } => match runner.store.list(limit) {
            Ok(tasks) => Response::ok(tasks),
            Err(error) => Response::err(error),
        },
        Request::ProposeOperation { prompt } => match propose::propose(&prompt) {
            Ok(proposal) => Response::ok(proposal),
            Err(error) => Response::err(error),
        },
        Request::McpRequest { prompt } => {
            match mcp_agent::run(&prompt, &runner.agent_identity, &runner.owner_pubkey) {
                Ok(result) => Response::ok(result),
                Err(error) => Response::err(error),
            }
        }
        Request::CancelTask { task_id } => match runner.store.request_cancel(&task_id) {
            Ok(task) => Response::ok(task),
            Err(error) => Response::err(error),
        },
        Request::ListRoutines => match routines.list() {
            Ok(routines) => Response::ok(routines),
            Err(error) => Response::err(error),
        },
        Request::AddRoutine {
            id,
            recipe_id,
            inputs,
            interval_seconds,
            agent_identity,
            enabled,
        } => match add_routine(
            runner,
            routines,
            id,
            recipe_id,
            inputs,
            interval_seconds,
            agent_identity,
            enabled,
        ) {
            Ok(routine) => Response::ok(routine),
            Err(error) => Response::err(error),
        },
        Request::SetRoutineEnabled {
            routine_id,
            enabled,
        } => match routines.set_enabled(&routine_id, enabled) {
            Ok(routine) => Response::ok(routine),
            Err(error) => Response::err(error),
        },
        Request::RemoveRoutine { routine_id } => match routines.remove(&routine_id) {
            Ok(()) => Response::ok(json!({ "removed": routine_id })),
            Err(error) => Response::err(error),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn add_routine(
    runner: &Runner,
    routines: &RoutineStore,
    id: String,
    recipe_id: String,
    inputs: Value,
    interval_seconds: u64,
    agent_identity: String,
    enabled: bool,
) -> Result<Routine, String> {
    if !valid_id(&id) {
        return Err("routine id must be lowercase ASCII letters, digits, '.', '_' or '-'".into());
    }
    if interval_seconds == 0 {
        return Err("routine interval must be at least one second".into());
    }
    let recipe = recipe::find(&recipe_id)?;
    let inputs = if inputs.is_null() { json!({}) } else { inputs };
    render::validate_inputs(&recipe.inputs_schema(), &inputs)?;
    let agent_pubkey = clients::signer_public_key(&agent_identity)?;
    if agent_pubkey == runner.owner_pubkey {
        return Err("a routine must not run as the owner identity".into());
    }
    if agent_pubkey == runner.agent_pubkey {
        return Err(
            "a routine needs its own identity, distinct from the interactive assistant".into(),
        );
    }
    if enabled && !recipe.operation_names().is_empty() {
        clients::require_grant(&agent_pubkey, &recipe.operation_names())?;
    }
    routines.add(Routine {
        id,
        recipe_id,
        inputs,
        interval_seconds,
        agent_identity,
        enabled,
        last_run_at: None,
        last_task_id: None,
        last_error: None,
        created_at: unix_time(),
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || "._-".contains(character)
        })
}

fn spawn_routine_scheduler(runner: Arc<Runner>, routines: Arc<RoutineStore>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let due = match routines.due(unix_time()) {
                Ok(due) => due,
                Err(error) => {
                    eprintln!("evermore-agentd: could not read routines: {error}");
                    continue;
                }
            };
            for routine in due {
                match run_routine(&runner, &routine) {
                    Ok(task_id) => {
                        let _ = routines.mark_run(&routine.id, Some(task_id), None);
                    }
                    Err(error) => {
                        eprintln!(
                            "evermore-agentd: routine '{}' did not start: {error}",
                            routine.id
                        );
                        let _ = routines.mark_run(&routine.id, None, Some(error));
                    }
                }
            }
        }
    });
}

fn run_routine(base: &Arc<Runner>, routine: &Routine) -> Result<String, String> {
    let agent_pubkey = clients::signer_public_key(&routine.agent_identity)?;
    if agent_pubkey == base.owner_pubkey {
        return Err("routine identity resolves to the owner identity".into());
    }
    if agent_pubkey == base.agent_pubkey {
        return Err("routine identity duplicates the interactive assistant".into());
    }
    let recipe = recipe::find(&routine.recipe_id)?;
    let runner = Arc::new(Runner {
        store: Arc::clone(&base.store),
        agent_identity: routine.agent_identity.clone(),
        agent_pubkey,
        owner_pubkey: base.owner_pubkey.clone(),
    });
    let task = runner.create_task(Some(routine.id.clone()), &recipe, routine.inputs.clone())?;
    let task_id = task.task_id.clone();
    std::thread::spawn(move || runner.run(task_id, recipe));
    Ok(task.task_id)
}

fn recipes_cli() -> Result<(), String> {
    let summaries: Vec<_> = recipe::load_all()
        .into_iter()
        .map(|recipe| recipe.summary())
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&summaries).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn run_cli() -> Result<(), String> {
    let mut arguments = std::env::args().skip(2);
    let recipe_id = arguments
        .next()
        .ok_or_else(|| "run requires a recipe id".to_string())?;
    let inputs: Value = arguments
        .next()
        .map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|error| format!("invalid JSON inputs: {error}"))?
        .unwrap_or_else(|| json!({}));
    let response = request(&Request::RunRecipe { recipe_id, inputs })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn propose_cli() -> Result<(), String> {
    let prompt = std::env::args().skip(2).collect::<Vec<_>>().join(" ");
    if prompt.trim().is_empty() {
        return Err("propose requires a request, for example: propose darken the theme".into());
    }
    let value = request(&Request::ProposeOperation { prompt })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn ask_cli() -> Result<(), String> {
    let prompt = std::env::args().skip(2).collect::<Vec<_>>().join(" ");
    if prompt.trim().is_empty() {
        return Err(
            "ask requires a request, for example: ask what is my current workspace?".into(),
        );
    }
    let value = request(&Request::McpRequest { prompt })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn tasks_cli() -> Result<(), String> {
    let tasks = match request(&Request::ListTasks { limit: 20 }) {
        Ok(value) => {
            serde_json::from_value::<Vec<Task>>(value).map_err(|error| error.to_string())?
        }
        Err(_) => agent::read_local_tasks(20).map_err(|error| error.to_string())?,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&tasks).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn status_cli() -> Result<(), String> {
    let task_id = std::env::args()
        .nth(2)
        .ok_or_else(|| "status requires a task id".to_string())?;
    let task = match request(&Request::TaskStatus {
        task_id: task_id.clone(),
    }) {
        Ok(value) => serde_json::from_value::<Task>(value).map_err(|error| error.to_string())?,
        Err(_) => agent::read_local_tasks(100)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|task| task.task_id == task_id)
            .ok_or_else(|| format!("unknown task '{task_id}'"))?,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&task).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn cancel_cli() -> Result<(), String> {
    let task_id = std::env::args()
        .nth(2)
        .ok_or_else(|| "cancel requires a task id".to_string())?;
    let value = request(&Request::CancelTask { task_id })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn routines_cli() -> Result<(), String> {
    let value = request(&Request::ListRoutines)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn agents_cli() -> Result<(), String> {
    let store = AgentStore::open()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&store.list()).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn agents_reload_cli() -> Result<(), String> {
    let value = request(&Request::ReloadAgents)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn routine_add_cli() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(2).collect();
    if arguments.len() < 2 {
        return Err(
            "routine-add requires ID and RECIPE, then --interval SECONDS --agent-identity NAME"
                .into(),
        );
    }
    let id = arguments[0].clone();
    let recipe_id = arguments[1].clone();
    let mut interval_seconds = None;
    let mut agent_identity = None;
    let mut inputs = json!({});
    let mut enabled = true;
    let mut index = 2;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--interval" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| "--interval requires a value".to_string())?;
                interval_seconds = Some(
                    value
                        .parse::<u64>()
                        .map_err(|error| format!("invalid interval: {error}"))?,
                );
                index += 2;
            }
            "--agent-identity" => {
                agent_identity = arguments.get(index + 1).cloned();
                index += 2;
            }
            "--inputs" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| "--inputs requires JSON".to_string())?;
                inputs = serde_json::from_str(value)
                    .map_err(|error| format!("invalid inputs JSON: {error}"))?;
                index += 2;
            }
            "--disabled" => {
                enabled = false;
                index += 1;
            }
            other => return Err(format!("unknown routine-add argument '{other}'")),
        }
    }
    let interval_seconds =
        interval_seconds.ok_or_else(|| "routine-add requires --interval SECONDS".to_string())?;
    let agent_identity =
        agent_identity.ok_or_else(|| "routine-add requires --agent-identity NAME".to_string())?;
    let value = request(&Request::AddRoutine {
        id,
        recipe_id,
        inputs,
        interval_seconds,
        agent_identity,
        enabled,
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn routine_toggle_cli(enabled: bool) -> Result<(), String> {
    let routine_id = std::env::args()
        .nth(2)
        .ok_or_else(|| "routine enable/disable requires a routine id".to_string())?;
    let value = request(&Request::SetRoutineEnabled {
        routine_id,
        enabled,
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn routine_remove_cli() -> Result<(), String> {
    let routine_id = std::env::args()
        .nth(2)
        .ok_or_else(|| "routine-remove requires a routine id".to_string())?;
    let value = request(&Request::RemoveRoutine { routine_id })?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn request(request: &Request) -> Result<Value, String> {
    let socket_path = agent::runtime_socket_path();
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|error| format!("couldn't connect to {}: {error}", socket_path.display()))?;
    let line = serde_json::to_string(request).map_err(|error| error.to_string())?;
    writeln!(stream, "{line}").map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(|error| error.to_string())?;
    match serde_json::from_str::<Response>(&response).map_err(|error| error.to_string())? {
        Response::Ok { data } => Ok(data),
        Response::Err { message } => Err(message),
    }
}

fn arg_value(name: &str) -> Option<String> {
    let mut arguments = std::env::args().skip(2);
    while let Some(argument) = arguments.next() {
        if argument == name {
            return arguments.next();
        }
    }
    None
}
