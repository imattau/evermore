use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use evermore_common::operations::{self, Access, Approval, Definition, Request, Response};
use serde::Serialize;
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2025-11-25";
const SERVER_NAME: &str = "evermore-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

struct Config {
    agent_identity: String,
    owner_pubkey: String,
}

fn main() {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("evermore-mcp: {error}");
            std::process::exit(2);
        }
    };
    let result = if std::env::args().any(|arg| arg == "--http") {
        serve_http(config)
    } else {
        serve(config)
    };
    if let Err(error) = result {
        eprintln!("evermore-mcp: {error}");
        std::process::exit(1);
    }
}

impl Config {
    fn from_env() -> Result<Self, String> {
        let agent_identity = std::env::var("EVERMORE_MCP_AGENT_IDENTITY").map_err(|_| {
            "set EVERMORE_MCP_AGENT_IDENTITY to the dedicated agent key name".to_string()
        })?;
        let owner_pubkey = std::env::var("EVERMORE_MCP_OWNER_PUBKEY").map_err(|_| {
            "set EVERMORE_MCP_OWNER_PUBKEY to the owner's public key hex".to_string()
        })?;
        if agent_identity.trim().is_empty() {
            return Err("EVERMORE_MCP_AGENT_IDENTITY cannot be empty".into());
        }
        if owner_pubkey.len() != 64 || !owner_pubkey.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("EVERMORE_MCP_OWNER_PUBKEY must be a 32-byte hex public key".into());
        }
        Ok(Self {
            agent_identity,
            owner_pubkey: owner_pubkey.to_ascii_lowercase(),
        })
    }
}

fn serve(config: Config) -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut initialized = false;
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let message = match serde_json::from_str::<Value>(&line) {
            Ok(message) => message,
            Err(error) => {
                write_message(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "id": null,
                        "error": { "code": -32700, "message": format!("Parse error: {error}") }
                    }),
                )?;
                continue;
            }
        };
        let response = process_message(&config, &message, &mut initialized);
        if let Some(response) = response {
            write_message(&mut stdout, response)?;
        }
    }
    Ok(())
}

fn process_message(config: &Config, message: &Value, initialized: &mut bool) -> Option<Value> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(rpc_error(
            message.get("id").cloned().unwrap_or(Value::Null),
            -32600,
            "Invalid Request",
        ));
    };
    let id = message.get("id").cloned();
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "initialize" if id.is_some() => {
            *initialized = true;
            Some(json!({
                "jsonrpc": "2.0",
                "id": id.unwrap(),
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION,
                        "title": "Evermore operations adapter"
                    },
                    "instructions": "Read calls use the shared operation catalogue. Writes submit scoped, owner-approval-gated requests and never execute directly in this adapter."
                }
            }))
        }
        "notifications/initialized" => {
            *initialized = true;
            None
        }
        "notifications/cancelled" | "notifications/progress" => None,
        "ping" if id.is_some() => Some(rpc_result(id.unwrap(), json!({}))),
        "tools/list" if id.is_some() => {
            if !*initialized {
                Some(rpc_error(id.unwrap(), -32002, "Server not initialized"))
            } else {
                Some(rpc_result(id.unwrap(), json!({ "tools": list_tools() })))
            }
        }
        "tools/call" if id.is_some() => {
            let id = id.unwrap();
            if !*initialized {
                Some(rpc_error(id, -32002, "Server not initialized"))
            } else {
                Some(rpc_result(id, call_tool(config, &params)))
            }
        }
        _ if id.is_none() => None,
        _ => Some(rpc_error(id.unwrap(), -32601, "Method not found")),
    }
}

fn serve_http(config: Config) -> Result<(), String> {
    let token = std::env::var("EVERMORE_MCP_HTTP_TOKEN").map_err(|_| {
        "set EVERMORE_MCP_HTTP_TOKEN to a random secret of at least 32 characters".to_string()
    })?;
    if token.len() < 32 {
        return Err("EVERMORE_MCP_HTTP_TOKEN must contain at least 32 characters".into());
    }
    let bind: SocketAddr = std::env::var("EVERMORE_MCP_HTTP_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8765".into())
        .parse()
        .map_err(|error| format!("invalid EVERMORE_MCP_HTTP_BIND: {error}"))?;
    if !bind.ip().is_loopback() {
        return Err("MCP HTTP may bind only to a loopback address".into());
    }
    let listener = TcpListener::bind(bind)
        .map_err(|error| format!("couldn't bind MCP HTTP endpoint: {error}"))?;
    let addr = listener.local_addr().map_err(|error| error.to_string())?;
    eprintln!("evermore-mcp HTTP listening at http://{addr}/mcp (loopback only)");
    let sessions = Arc::new(Mutex::new(HashMap::<String, bool>::new()));
    let config = Arc::new(config);
    let token = Arc::new(token);
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let config = Arc::clone(&config);
                let token = Arc::clone(&token);
                let sessions = Arc::clone(&sessions);
                std::thread::spawn(move || {
                    if let Err(error) = handle_http(stream, &config, &token, addr, &sessions) {
                        eprintln!("evermore-mcp HTTP request failed: {error}");
                    }
                });
            }
            Err(error) => eprintln!("evermore-mcp HTTP accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_http(
    mut stream: TcpStream,
    config: &Config,
    token: &str,
    bind: SocketAddr,
    sessions: &Mutex<HashMap<String, bool>>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut request_line = String::new();
    if !read_line_limited(&mut reader, &mut request_line, 4096).map_err(|e| e.to_string())? {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");
    if version != "HTTP/1.1" {
        return write_http(&mut stream, 400, "Bad Request", &[], "");
    }
    let mut headers = HashMap::<String, String>::new();
    let mut header_bytes = 0usize;
    loop {
        let mut line = String::new();
        if !read_line_limited(&mut reader, &mut line, 8192).map_err(|e| e.to_string())? {
            return write_http(&mut stream, 400, "Bad Request", &[], "");
        }
        let count = line.len();
        header_bytes += count;
        if header_bytes > 16 * 1024 {
            return write_http(&mut stream, 431, "Request Header Fields Too Large", &[], "");
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return write_http(&mut stream, 400, "Bad Request", &[], "");
        };
        let key = name.trim().to_ascii_lowercase();
        if headers.insert(key, value.trim().to_string()).is_some() {
            return write_http(&mut stream, 400, "Bad Request", &[], "duplicate header");
        }
    }
    if headers.contains_key("transfer-encoding") {
        return write_http(
            &mut stream,
            400,
            "Bad Request",
            &[],
            "Transfer-Encoding is unsupported",
        );
    }

    let valid_hosts = local_hosts(bind);
    if !headers.get("host").is_some_and(|host| {
        valid_hosts
            .iter()
            .any(|allowed| host.eq_ignore_ascii_case(allowed))
    }) {
        return write_http(&mut stream, 403, "Forbidden", &[], "invalid Host");
    }
    if let Some(origin) = headers.get("origin") {
        let valid_origins = local_origins(bind);
        if !valid_origins
            .iter()
            .any(|allowed| origin.eq_ignore_ascii_case(allowed))
        {
            return write_http(&mut stream, 403, "Forbidden", &[], "invalid Origin");
        }
    }
    if !headers.get("authorization").is_some_and(|value| {
        constant_time_equal(value.as_bytes(), format!("Bearer {token}").as_bytes())
    }) {
        return write_http(
            &mut stream,
            401,
            "Unauthorized",
            &[("WWW-Authenticate", "Bearer")],
            "authentication required",
        );
    }
    if path != "/mcp" {
        return write_http(&mut stream, 404, "Not Found", &[], "");
    }
    match method {
        "GET" => {
            return write_http(
                &mut stream,
                405,
                "Method Not Allowed",
                &[("Allow", "POST")],
                "",
            );
        }
        "DELETE" => {
            return write_http(
                &mut stream,
                405,
                "Method Not Allowed",
                &[("Allow", "POST")],
                "",
            );
        }
        "POST" => {}
        _ => {
            return write_http(
                &mut stream,
                405,
                "Method Not Allowed",
                &[("Allow", "POST")],
                "",
            );
        }
    }
    if !headers.get("accept").is_some_and(|value| {
        value.contains("application/json") && value.contains("text/event-stream")
    }) {
        return write_http(
            &mut stream,
            406,
            "Not Acceptable",
            &[],
            "Accept must include application/json and text/event-stream",
        );
    }
    if !headers.get("content-type").is_some_and(|value| {
        value
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("application/json")
    }) {
        return write_http(
            &mut stream,
            415,
            "Unsupported Media Type",
            &[],
            "Content-Type must be application/json",
        );
    }
    let body_length = match headers
        .get("content-length")
        .and_then(|s| s.parse::<usize>().ok())
    {
        Some(length) if length <= 1024 * 1024 => length,
        _ => {
            return write_http(
                &mut stream,
                413,
                "Payload Too Large",
                &[],
                "invalid or oversized Content-Length",
            );
        }
    };
    let body = if body_length == 0 {
        return write_http(&mut stream, 400, "Bad Request", &[], "empty JSON-RPC body");
    } else {
        let mut body = vec![0; body_length];
        if reader.read_exact(&mut body).is_err() {
            return write_http(
                &mut stream,
                400,
                "Bad Request",
                &[],
                "incomplete request body",
            );
        }
        body
    };
    let message: Value = match serde_json::from_slice::<Value>(&body) {
        Ok(message) if message.is_object() => message,
        _ => {
            return write_http(
                &mut stream,
                400,
                "Bad Request",
                &[],
                "body must be one JSON-RPC object",
            );
        }
    };
    let has_method = message.get("method").and_then(Value::as_str).is_some();
    if !has_method
        && message.get("id").is_some()
        && (message.get("result").is_some() || message.get("error").is_some())
    {
        return write_http(&mut stream, 202, "Accepted", &[], "");
    }
    let initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
    let session_id = headers.get("mcp-session-id");
    if initialize {
        if session_id.is_some() {
            return write_http(
                &mut stream,
                400,
                "Bad Request",
                &[],
                "initialize must not include MCP-Session-Id",
            );
        }
    } else {
        let Some(session_id) = session_id else {
            return write_http(
                &mut stream,
                400,
                "Bad Request",
                &[],
                "MCP-Session-Id is required",
            );
        };
        if headers.get("mcp-protocol-version").map(String::as_str) != Some(PROTOCOL_VERSION) {
            return write_http(
                &mut stream,
                400,
                "Bad Request",
                &[],
                "unsupported or missing MCP-Protocol-Version",
            );
        }
        if !sessions
            .lock()
            .map_err(|_| "session lock poisoned")?
            .contains_key(session_id)
        {
            return write_http(&mut stream, 404, "Not Found", &[], "unknown MCP session");
        }
    }
    let mut initialized = true;
    let response = process_message(config, &message, &mut initialized);
    if initialize {
        let id = new_session_id()?;
        let mut sessions = sessions.lock().map_err(|_| "session lock poisoned")?;
        if sessions.len() >= 1024 {
            return write_http(
                &mut stream,
                503,
                "Service Unavailable",
                &[],
                "session capacity reached",
            );
        }
        sessions.insert(id.clone(), true);
        drop(sessions);
        if let Some(response) = response {
            let body = serde_json::to_string(&response).map_err(|e| e.to_string())?;
            return write_http(
                &mut stream,
                200,
                "OK",
                &[
                    ("Content-Type", "application/json"),
                    ("MCP-Protocol-Version", PROTOCOL_VERSION),
                    ("MCP-Session-Id", &id),
                ],
                &body,
            );
        }
        return write_http(
            &mut stream,
            400,
            "Bad Request",
            &[],
            "initialize must be a request",
        );
    }
    if response.is_none() || (message.get("id").is_none() && message.get("method").is_none()) {
        return write_http(&mut stream, 202, "Accepted", &[], "");
    }
    if let Some(response) = response {
        let body = serde_json::to_string(&response).map_err(|e| e.to_string())?;
        return write_http(
            &mut stream,
            200,
            "OK",
            &[
                ("Content-Type", "application/json"),
                ("MCP-Protocol-Version", PROTOCOL_VERSION),
            ],
            &body,
        );
    }
    write_http(&mut stream, 202, "Accepted", &[], "")
}

fn read_line_limited(
    reader: &mut impl BufRead,
    output: &mut String,
    limit: usize,
) -> io::Result<bool> {
    output.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(!output.is_empty());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if output.len() + take > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP line too long",
            ));
        }
        let chunk = std::str::from_utf8(&available[..take]).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "HTTP headers must be UTF-8")
        })?;
        output.push_str(chunk);
        reader.consume(take);
        if newline.is_some() {
            return Ok(true);
        }
    }
}

fn local_hosts(bind: SocketAddr) -> Vec<String> {
    let port = bind.port();
    vec![
        bind.to_string(),
        format!("localhost:{port}"),
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
    ]
}

fn local_origins(bind: SocketAddr) -> Vec<String> {
    local_hosts(bind)
        .into_iter()
        .map(|host| format!("http://{host}"))
        .collect()
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn new_session_id() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| format!("secure session ID generation failed: {e}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn write_http(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<(), String> {
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    )
    .map_err(|e| e.to_string())?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").map_err(|e| e.to_string())?;
    }
    write!(stream, "\r\n{body}").map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())
}

fn list_tools() -> Vec<Value> {
    let mut tools: Vec<Value> = operations::catalog()
        .into_iter()
        .filter(|definition| {
            definition.access == Access::Read || definition.approval == Approval::Required
        })
        .map(operation_tool)
        .collect();
    tools.push(json!({
        "name": "mcp_status",
        "description": "Report adapter reachability and the configured local agent identity. This does not require the operation daemon or relay.",
        "inputSchema": empty_input_schema(),
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false }
    }));
    tools.push(json!({
        "name": "op_status",
        "description": "Read the durable status and outcome for an operation request by operation_id.",
        "inputSchema": json!({
            "type": "object",
            "properties": { "operation_id": { "type": "string", "minLength": 1 } },
            "required": ["operation_id"],
            "additionalProperties": false
        }),
        "annotations": { "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false }
    }));
    tools.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    tools
}

fn operation_tool(definition: Definition) -> Value {
    let write = definition.access == Access::Write;
    let mut description = definition.description.clone();
    if write {
        description.push_str(" This submits an approval-gated request; it does not execute until the owner approves it.");
    }
    description.push_str(&format!(
        " Required scoped operation grant: '{}'.",
        definition.name
    ));
    if !definition.required_grants.is_empty() {
        description.push_str(&format!(
            " Catalogue policy labels: {}.",
            definition.required_grants.join(", ")
        ));
    }
    let mut tool = json!({
        "name": definition.name,
        "description": description,
        "inputSchema": definition.input_schema,
        "annotations": {
            "readOnlyHint": !write,
            "destructiveHint": write,
            "openWorldHint": false
        }
    });
    if !write {
        tool["outputSchema"] = definition.output_schema;
    }
    tool
}

fn call_tool(config: &Config, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return tool_error("tools/call is missing a string tool name");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return tool_error("tool arguments must be a JSON object");
    }
    if name == "mcp_status" && !arguments.as_object().is_some_and(serde_json::Map::is_empty) {
        return tool_error("mcp_status accepts no arguments");
    }
    if name == "op_status"
        && (arguments.as_object().is_none_or(|object| {
            object.len() != 1
                || !arguments
                    .get("operation_id")
                    .and_then(Value::as_str)
                    .is_some_and(|operation_id| !operation_id.is_empty())
        }))
    {
        return tool_error("op_status requires only a non-empty operation_id");
    }
    let result = match name {
        "mcp_status" => mcp_status(config),
        "op_status" => operation_status(config, &arguments),
        _ => call_operation(config, name, &arguments),
    };
    match result {
        Ok(data) => tool_success(data),
        Err(error) => tool_error(&error),
    }
}

fn mcp_status(config: &Config) -> Result<Value, String> {
    let status = match agent_pubkey(config) {
        Ok(pubkey) if pubkey != config.owner_pubkey => json!({
            "adapter_reachable": true,
            "caller_identity": config.agent_identity,
            "caller_pubkey": pubkey,
            "caller_identity_recognized": true
        }),
        _ => json!({
            "adapter_reachable": true,
            "caller_identity": config.agent_identity,
            "caller_pubkey": null,
            "caller_identity_recognized": false
        }),
    };
    Ok(status)
}

fn operation_status(config: &Config, arguments: &Value) -> Result<Value, String> {
    let operation_id = arguments
        .get("operation_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "op_status requires operation_id".to_string())?;
    let mut record = match operations_request(&Request::OperationStatus {
        operation_id: operation_id.to_string(),
    }) {
        Ok(record) => record,
        Err(daemon_error) => operations::read_local_record(operation_id)
            .map_err(|journal_error| format!("operation daemon unavailable ({daemon_error}); offline journal lookup failed ({journal_error})"))?,
    };
    let caller_pubkey = agent_pubkey(config)?;
    if record.get("actor_pubkey").and_then(Value::as_str) != Some(&caller_pubkey) {
        return Err("unknown operation_id".into());
    }
    if let Some(definition) = record
        .get("operation")
        .and_then(Value::as_str)
        .and_then(operations::find)
    {
        if let Some(arguments) = record.get("arguments").cloned() {
            record["arguments"] = operations::redact_result(&definition, arguments);
        }
        if let Some(result) = record
            .get("result")
            .cloned()
            .filter(|value| !value.is_null())
        {
            record["result"] = operations::redact_result(&definition, result);
        }
    }
    Ok(record)
}

fn call_operation(config: &Config, name: &str, arguments: &Value) -> Result<Value, String> {
    let definition = operations::find(name).ok_or_else(|| format!("unknown tool '{name}'"))?;
    let normalized_arguments = operations::canonicalize_input(name, arguments)?;
    let arguments = &normalized_arguments;
    require_active_grant(config, name)?;
    if definition.access == Access::Read {
        return operations_request(&Request::Invoke {
            name: name.to_string(),
            arguments: arguments.clone(),
        });
    }
    if definition.approval != Approval::Required {
        return Err(format!(
            "write operation '{name}' has no supported approval path"
        ));
    }

    let plaintext = json!({ "operation": name, "arguments": arguments }).to_string();
    let ciphertext = signer_request(&evermore_common::Request::Nip44Encrypt {
        identity: config.agent_identity.clone(),
        recipient_pubkey: config.owner_pubkey.clone(),
        plaintext,
    })?
    .get("ciphertext")
    .and_then(Value::as_str)
    .map(str::to_string)
    .ok_or_else(|| "signer response is missing ciphertext".to_string())?;
    let event = signer_request(&evermore_common::Request::SignEvent {
        identity: config.agent_identity.clone(),
        kind: 1,
        content: ciphertext,
        tags: vec![
            vec!["t".into(), "evermore-operation-request".into()],
            vec!["p".into(), config.owner_pubkey.clone()],
        ],
        created_at: None,
    })?;
    let record = operations_request(&Request::SubmitSignedRequest { event })?;
    Ok(json!({
        "operation_id": record.get("operation_id").cloned().unwrap_or(Value::Null),
        "operation": name,
        "status": record.get("status").cloned().unwrap_or(Value::Null),
        "approval_required": true
    }))
}

fn agent_pubkey(config: &Config) -> Result<String, String> {
    signer_request(&evermore_common::Request::GetPublicKey {
        identity: config.agent_identity.clone(),
    })?
    .get("pubkey_hex")
    .and_then(Value::as_str)
    .map(str::to_ascii_lowercase)
    .ok_or_else(|| "signer response is missing pubkey_hex".to_string())
}

fn require_active_grant(config: &Config, operation: &str) -> Result<(), String> {
    let agent_pubkey = agent_pubkey(config)?;
    if agent_pubkey == config.owner_pubkey {
        return Err("MCP agent identity must be separate from the owner identity".into());
    }
    let grants = operations_request(&Request::ListGrants)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let has_grant = grants.as_array().is_some_and(|items| {
        items.iter().any(|grant| {
            grant.get("agent_pubkey").and_then(Value::as_str) == Some(&agent_pubkey)
                && grant.get("grant_published").and_then(Value::as_bool) == Some(true)
                && grant.get("revoked").and_then(Value::as_bool) == Some(false)
                && grant
                    .get("expires_at")
                    .and_then(Value::as_u64)
                    .is_some_and(|expiry| expiry > now)
                && grant
                    .get("allowed_operations")
                    .and_then(Value::as_array)
                    .is_some_and(|operations| {
                        operations
                            .iter()
                            .any(|name| name.as_str() == Some(operation))
                    })
        })
    });
    if has_grant {
        Ok(())
    } else {
        Err(format!("agent has no active grant for '{operation}'"))
    }
}

fn signer_request<T: Serialize>(request: &T) -> Result<Value, String> {
    let value = serde_json::to_value(request).map_err(|error| error.to_string())?;
    let response: evermore_common::Response =
        send_request(&evermore_common::runtime_socket_path(), &value)?;
    match response {
        evermore_common::Response::Ok { data } => Ok(data),
        evermore_common::Response::Err { message } => Err(message),
    }
}

fn operations_request(request: &Request) -> Result<Value, String> {
    let value = serde_json::to_value(request).map_err(|error| error.to_string())?;
    let response: Response = send_request(&operations::runtime_socket_path(), &value)?;
    match response {
        Response::Ok { data } => Ok(data),
        Response::Err { message } => Err(message),
    }
}

fn send_request<R: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    request: &Value,
) -> Result<R, String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("couldn't connect to {}: {error}", path.display()))?;
    writeln!(stream, "{}", request).map_err(|error| error.to_string())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&line).map_err(|error| error.to_string())
}

fn empty_input_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn tool_success(data: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": data.to_string() }],
        "structuredContent": data,
        "isError": false
    })
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true
    })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn write_message(writer: &mut impl Write, message: Value) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, &message).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}
