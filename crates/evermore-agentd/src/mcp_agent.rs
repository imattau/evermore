use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use evermore_common::ai::{Message, Role};
use serde_json::{Value, json};

use crate::infer;

const MAX_TOOL_CALLS: usize = 3;
const MAX_COMPLETION_TOKENS: u32 = 512;
const MAX_TOOL_RESULT_CHARS: usize = 6000;
const MCP_GRAMMAR: &str = include_str!("../../../tools/adapter-training/mcp-tool-call.gbnf");
const ACTION_GRAMMAR: &str = r#"
root ::= call | refusal | final
call ::= "{\"name\":" string ",\"arguments\":" object "}"
refusal ::= "{\"refusal\":" string "}"
final ::= "{\"final\":" string "}"
object ::= "{" (pair ("," pair)*)? "}"
pair ::= string ":" value
value ::= object | array | string | number | boolean | "null"
array ::= "[" (value ("," value)*)? "]"
string ::= "\"" char* "\""
char ::= [^"\\] | "\\" (["\\/bfnrt] | "u" hex hex hex hex)
hex ::= [0-9a-fA-F]
number ::= "-"? ("0" | [1-9] [0-9]*) ("." [0-9]+)? ([eE] [+-]? [0-9]+)?
boolean ::= "true" | "false"
"#;

pub fn run(prompt: &str, identity: &str, owner_pubkey: &str) -> Result<Value, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("describe what you want Evermore to do first".into());
    }
    if prompt.chars().count() > 4000 {
        return Err("request is too long; keep it under 4000 characters".into());
    }

    let mut child = Command::new(
        std::env::var("EVERMORE_MCP_COMMAND").unwrap_or_else(|_| "evermore-mcp".into()),
    )
    .env("EVERMORE_MCP_AGENT_IDENTITY", identity)
    .env("EVERMORE_MCP_OWNER_PUBKEY", owner_pubkey)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .map_err(|error| format!("couldn't start evermore-mcp: {error}"))?;

    let result = run_session(&mut child, prompt);
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn run_session(child: &mut Child, prompt: &str) -> Result<Value, String> {
    let mut input = child.stdin.take().ok_or("MCP stdin unavailable")?;
    let output = child.stdout.take().ok_or("MCP stdout unavailable")?;
    let mut output = BufReader::new(output);
    let mut next_id = 1_u64;
    let initialized = rpc(
        &mut input,
        &mut output,
        &mut next_id,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "evermore-agentd", "version": env!("CARGO_PKG_VERSION") }
        }),
    )?;
    if initialized.get("error").is_some() {
        return Err(format!("MCP initialization failed: {initialized}"));
    }
    notify(&mut input, "notifications/initialized", json!({}))?;
    let listed = rpc(
        &mut input,
        &mut output,
        &mut next_id,
        "tools/list",
        json!({}),
    )?;
    let tools = listed
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("MCP returned an invalid tool catalogue: {listed}"))?;
    if tools.is_empty() {
        return Err("MCP returned no available tools".into());
    }

    let model = infer::select_task_model("mcp")?;
    let instruction = "You are Evermore's local MCP tool user. Select only from the available tools below. Use exact names and schema-valid arguments. A write submits an owner-approval-gated request; never claim it is executed. After seeing tool results, call another tool only if needed, otherwise return {\"final\":\"concise answer\"}. Refuse unsupported requests with {\"refusal\":\"reason\"}.";
    let mut messages = vec![
        Message {
            role: Role::System,
            content: instruction.to_string(),
        },
        Message {
            role: Role::User,
            content: prompt.to_string(),
        },
    ];
    let mut calls = 0;
    let mut write_submitted = false;
    let mut retrieval_text = prompt.to_string();
    loop {
        let candidates = shortlist(&retrieval_text, tools);
        messages[0].content = format!(
            "{instruction}\nAvailable tools: {}",
            serde_json::to_string(&candidates).map_err(|e| e.to_string())?
        );
        let response = infer::chat(
            &model,
            messages.clone(),
            Some(
                if calls == 0 {
                    MCP_GRAMMAR
                } else {
                    ACTION_GRAMMAR
                }
                .into(),
            ),
            MAX_COMPLETION_TOKENS,
        )?;
        let action: Value = serde_json::from_str(response.text.trim())
            .map_err(|error| format!("MCP model returned invalid JSON: {error}"))?;
        if let Some(reason) = action.get("refusal").and_then(Value::as_str) {
            return Ok(
                json!({"status":"refused", "reason":reason, "model":model, "tool_calls":calls}),
            );
        }
        if let Some(answer) = action.get("final").and_then(Value::as_str) {
            if calls == 0 {
                return Err("MCP model returned a final answer without calling a tool".into());
            }
            return Ok(
                json!({"status":"completed", "answer":answer, "model":model, "tool_calls":calls}),
            );
        }
        if calls >= MAX_TOOL_CALLS {
            return Err("MCP tool-call limit reached".into());
        }
        if write_submitted {
            return Err("stopped after submitting an approval-gated write request".into());
        }
        let name = action
            .get("name")
            .and_then(Value::as_str)
            .ok_or("MCP model response has no tool name")?;
        let args = action
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !args.is_object() {
            return Err("MCP arguments must be a JSON object".into());
        }
        let tool = candidates
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| format!("MCP model selected unlisted tool '{name}'"))?;
        let read_only = tool
            .pointer("/annotations/readOnlyHint")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("MCP tool '{name}' has no read-only annotation"))?;
        write_submitted = !read_only;
        messages.push(Message {
            role: Role::Assistant,
            content: response.text,
        });
        let result = rpc(
            &mut input,
            &mut output,
            &mut next_id,
            "tools/call",
            json!({"name":name,"arguments":args}),
        )?;
        let result = result.get("result").cloned().unwrap_or(result);
        calls += 1;
        let serialized = serde_json::to_string(&result).map_err(|e| e.to_string())?;
        let tool_text = truncate_for_prompt(&serialized);
        retrieval_text.push('\n');
        retrieval_text.push_str(&tool_text);
        messages.push(Message { role: Role::User, content: format!("Tool {name} returned: {tool_text}. If a write was submitted, explain it is awaiting owner approval. Otherwise continue only if another tool is required; return a final answer when done.") });
        if write_submitted {
            let status = if result.get("isError").and_then(Value::as_bool) == Some(true) {
                "tool_error"
            } else {
                "awaiting_approval"
            };
            return Ok(
                json!({"status":status, "tool":name, "result":result, "model":model, "tool_calls":calls}),
            );
        }
    }
}

fn truncate_for_prompt(text: &str) -> String {
    let mut chars = text.chars();
    let preview: String = chars.by_ref().take(MAX_TOOL_RESULT_CHARS).collect();
    if chars.next().is_some() {
        format!("{preview}[tool result truncated for model context]")
    } else {
        preview
    }
}

fn shortlist(request: &str, tools: &[Value]) -> Vec<Value> {
    let request_terms = terms(request);
    let mut ranked: Vec<(usize, &Value)> = tools
        .iter()
        .map(|tool| {
            let searchable = format!(
                "{} {}",
                tool.get("name").and_then(Value::as_str).unwrap_or_default(),
                tool.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
            (
                terms(&searchable).intersection(&request_terms).count(),
                tool,
            )
        })
        .collect();
    ranked.sort_by(|(score_a, tool_a), (score_b, tool_b)| {
        score_b.cmp(score_a).then_with(|| {
            tool_a
                .get("name")
                .and_then(Value::as_str)
                .cmp(&tool_b.get("name").and_then(Value::as_str))
        })
    });
    let mut selected: Vec<Value> = ranked
        .iter()
        .take(6)
        .map(|(_, tool)| (*tool).clone())
        .collect();
    selected.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    selected
}

fn terms(text: &str) -> std::collections::BTreeSet<String> {
    const STOP: &[&str] = &[
        "about", "after", "also", "and", "are", "for", "from", "help", "into", "make", "please",
        "that", "the", "this", "with", "would",
    ];
    text.split(|character: char| !character.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|term| term.chars().count() > 2 && !STOP.contains(&term.as_str()))
        .collect()
}

fn rpc(
    input: &mut ChildStdin,
    output: &mut BufReader<ChildStdout>,
    id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let current = *id;
    *id += 1;
    serde_json::to_writer(
        &mut *input,
        &json!({"jsonrpc":"2.0","id":current,"method":method,"params":params}),
    )
    .map_err(|e| e.to_string())?;
    input
        .write_all(b"\n")
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    output.read_line(&mut line).map_err(|e| e.to_string())?;
    if line.is_empty() {
        return Err("MCP server closed its output".into());
    }
    let value: Value =
        serde_json::from_str(&line).map_err(|e| format!("invalid MCP response: {e}"))?;
    if value.get("id").and_then(Value::as_u64) != Some(current) {
        return Err("MCP server returned a mismatched request id".into());
    }
    Ok(value)
}

fn notify(input: &mut ChildStdin, method: &str, params: Value) -> Result<(), String> {
    serde_json::to_writer(
        &mut *input,
        &json!({"jsonrpc":"2.0","method":method,"params":params}),
    )
    .map_err(|e| e.to_string())?;
    input
        .write_all(b"\n")
        .and_then(|_| input.flush())
        .map_err(|e| e.to_string())
}
