use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use evermore_common::operations::{self, Definition, Request, Response};

pub fn fetch_catalog() -> Result<Vec<Definition>, String> {
    let socket_path = operations::runtime_socket_path();
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| format!("couldn't connect to {}: {e}", socket_path.display()))?;
    let request = serde_json::to_string(&Request::Catalog).map_err(|e| e.to_string())?;
    writeln!(stream, "{request}").map_err(|e| e.to_string())?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let response: Response = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    match response {
        Response::Ok { data } => serde_json::from_value(data).map_err(|e| e.to_string()),
        Response::Err { message } => Err(message),
    }
}
