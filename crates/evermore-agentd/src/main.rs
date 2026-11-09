use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;

use evermore_agentd::{clients, propose, recipe, runner::Runner, store::TaskStore};
use evermore_common::agent::{self, AgentHealth, Request, Response, Task};
use serde_json::{Value, json};

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("serve") => serve().map_err(|error| error.to_string()),
        Some("recipes") => recipes_cli(),
        Some("run") => run_cli(),
        Some("propose") => propose_cli(),
        Some("tasks") => tasks_cli(),
        Some("status") => status_cli(),
        Some("cancel") => cancel_cli(),
        _ => {
            eprintln!("usage: evermore-agentd <serve|recipes|run|propose|tasks|status|cancel>");
            eprintln!("  serve --agent-identity NAME [--owner-pubkey HEX]");
            eprintln!("  serve    run the deterministic recipe task runner");
            eprintln!("  recipes  list built-in and local recipes");
            eprintln!("  run      run a recipe: run RECIPE [JSON_INPUTS]");
            eprintln!("  propose  generate one validated write proposal: propose REQUEST");
            eprintln!("  tasks    list recent tasks");
            eprintln!("  status   show a task: status TASK_ID");
            eprintln!("  cancel   request task cancellation: cancel TASK_ID");
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
    let store = Arc::new(TaskStore::open().map_err(std::io::Error::other)?);
    let runner = Arc::new(Runner {
        store: Arc::clone(&store),
        agent_pubkey: agent_pubkey.clone(),
        owner_pubkey: owner_pubkey.clone(),
    });

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
                if let Err(error) = handle_connection(stream, &runner) {
                    eprintln!("evermore-agentd: request failed: {error}");
                }
            }
            Err(error) => eprintln!("evermore-agentd: accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(mut stream: UnixStream, runner: &Arc<Runner>) -> std::io::Result<()> {
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    if line.trim().is_empty() {
        return Ok(());
    }
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) => handle_request(request, runner),
        Err(error) => Response::err(format!("invalid request: {error}")),
    };
    let mut output = serde_json::to_string(&response)
        .unwrap_or_else(|_| r#"{"status":"err","message":"serialization failed"}"#.to_string());
    output.push('\n');
    stream.write_all(output.as_bytes())?;
    stream.flush()
}

fn handle_request(request: Request, runner: &Arc<Runner>) -> Response {
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
                Ok(recipe) => match runner.create_task(&recipe, inputs) {
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
        Request::CancelTask { task_id } => match runner.store.request_cancel(&task_id) {
            Ok(task) => Response::ok(task),
            Err(error) => Response::err(error),
        },
    }
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
