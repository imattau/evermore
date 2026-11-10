use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use evermore_common::agent::{self, RecipeSummary, Request, Response, Task};
use serde_json::Value;

pub fn fetch_recipes() -> Result<Vec<RecipeSummary>, String> {
    match request(&Request::ListRecipes) {
        Ok(value) => {
            serde_json::from_value(value).map_err(|error| format!("invalid recipe list: {error}"))
        }
        Err(error) => Err(format!(
            "Agent runner unavailable; local recipes can still be inspected with evermore-agentd recipes: {error}"
        )),
    }
}

pub fn fetch_tasks(limit: usize) -> Result<Vec<Task>, String> {
    match request(&Request::ListTasks { limit }) {
        Ok(value) => serde_json::from_value(value).map_err(|error| error.to_string()),
        Err(_) => agent::read_local_tasks(limit).map_err(|error| error.to_string()),
    }
}

pub fn run_recipe(recipe_id: &str, inputs: Value) -> Result<Task, String> {
    let value = request(&Request::RunRecipe {
        recipe_id: recipe_id.to_string(),
        inputs,
    })?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

pub fn cancel_task(task_id: &str) -> Result<Task, String> {
    let value = request(&Request::CancelTask {
        task_id: task_id.to_string(),
    })?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

pub fn propose_operation(prompt: &str) -> Result<evermore_common::agent::Proposal, String> {
    let value = request(&Request::ProposeOperation {
        prompt: prompt.to_string(),
    })?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

pub fn fetch_agents() -> Result<Vec<evermore_common::agent::AgentView>, String> {
    match request(&Request::ListAgents) {
        Ok(value) => {
            serde_json::from_value(value).map_err(|error| format!("invalid agent list: {error}"))
        }
        Err(error) => Err(error),
    }
}

pub fn fetch_templates() -> Result<Vec<evermore_common::agent::AgentTemplate>, String> {
    let value = request(&Request::ListAgentTemplates)?;
    serde_json::from_value(value).map_err(|error| format!("invalid agent template list: {error}"))
}

pub fn reload_agents() -> Result<evermore_common::agent::AgentReload, String> {
    let value = request(&Request::ReloadAgents)?;
    serde_json::from_value(value).map_err(|error| format!("invalid agent reload: {error}"))
}

pub fn run_agent_job(agent_id: &str, job_id: &str) -> Result<Task, String> {
    let value = request(&Request::RunAgentJob {
        agent_id: agent_id.to_string(),
        job_id: job_id.to_string(),
    })?;
    serde_json::from_value(value).map_err(|error| error.to_string())
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
