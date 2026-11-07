//! Minimal loopback HTTP client for the supervised `llama-server`.
//!
//! Kept hand-rolled like the rest of the workspace: the only needs are JSON
//! requests and a streaming SSE body, and avoiding a full HTTP stack keeps
//! the daemon's dependency surface small.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::Value;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const READ_TIMEOUT: Duration = Duration::from_secs(300);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct HttpResponse<R> {
    pub status: u16,
    pub body: HttpBody<R>,
}

pub struct HttpBody<R> {
    reader: R,
    chunked: bool,
    remaining: Option<u64>,
    chunk_remaining: usize,
    need_chunk_terminator: bool,
    done: bool,
}

impl<R: BufRead + Read> Read for HttpBody<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.done {
            return Ok(0);
        }
        if self.chunked {
            loop {
                if self.need_chunk_terminator {
                    let mut terminator = [0_u8; 2];
                    self.reader.read_exact(&mut terminator)?;
                    if terminator != *b"\r\n" {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid HTTP chunk terminator",
                        ));
                    }
                    self.need_chunk_terminator = false;
                }
                if self.chunk_remaining > 0 {
                    let size = buffer.len().min(self.chunk_remaining);
                    let count = self.reader.read(&mut buffer[..size])?;
                    if count == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "truncated HTTP chunk",
                        ));
                    }
                    self.chunk_remaining -= count;
                    if self.chunk_remaining == 0 {
                        self.need_chunk_terminator = true;
                    }
                    return Ok(count);
                }
                let mut line = String::new();
                self.reader.read_line(&mut line)?;
                let chunk_size = line.trim().split(';').next().unwrap_or_default();
                let size = usize::from_str_radix(chunk_size, 16)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                if size == 0 {
                    loop {
                        line.clear();
                        self.reader.read_line(&mut line)?;
                        if line == "\r\n" || line == "\n" || line.is_empty() {
                            break;
                        }
                    }
                    self.done = true;
                    return Ok(0);
                }
                self.chunk_remaining = size;
            }
        }
        if let Some(remaining) = self.remaining {
            if remaining == 0 {
                self.done = true;
                return Ok(0);
            }
            let size = buffer.len().min(remaining as usize);
            let count = self.reader.read(&mut buffer[..size])?;
            self.remaining = Some(remaining.saturating_sub(count as u64));
            return Ok(count);
        }
        self.reader.read(buffer)
    }
}

pub fn open_response(
    addr: SocketAddr,
    api_key: Option<&str>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<HttpResponse<BufReader<TcpStream>>, String> {
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|error| format!("couldn't connect to llama-server at {addr}: {error}"))?;
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(WRITE_TIMEOUT))
        .map_err(|error| error.to_string())?;

    let body_bytes = body
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|error| error.to_string())?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAccept: text/event-stream, application/json\r\nConnection: close\r\n"
    )
    .map_err(|error| error.to_string())?;
    if let Some(key) = api_key {
        write!(stream, "Authorization: Bearer {key}\r\n").map_err(|error| error.to_string())?;
    }
    if let Some(bytes) = &body_bytes {
        write!(
            stream,
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            bytes.len()
        )
        .map_err(|error| error.to_string())?;
    }
    stream
        .write_all(b"\r\n")
        .map_err(|error| error.to_string())?;
    if let Some(bytes) = &body_bytes {
        stream.write_all(bytes).map_err(|error| error.to_string())?;
    }

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|error| error.to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "llama-server returned an invalid HTTP status".to_string())?
        .parse::<u16>()
        .map_err(|error| error.to_string())?;
    let mut chunked = false;
    let mut remaining = None;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("transfer-encoding") {
                chunked = value.to_ascii_lowercase().contains("chunked");
            } else if key.eq_ignore_ascii_case("content-length") {
                remaining = Some(
                    value
                        .trim()
                        .parse::<u64>()
                        .map_err(|error| error.to_string())?,
                );
            }
        }
    }
    Ok(HttpResponse {
        status,
        body: HttpBody {
            reader,
            chunked,
            remaining,
            chunk_remaining: 0,
            need_chunk_terminator: false,
            done: false,
        },
    })
}

pub fn request_json(
    addr: SocketAddr,
    api_key: Option<&str>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    let mut response = open_response(addr, api_key, method, path, body)?;
    let mut text = String::new();
    response
        .body
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "llama-server returned HTTP {}: {}",
            response.status,
            truncate(&text)
        ));
    }
    serde_json::from_str(&text).map_err(|error| format!("invalid llama-server response: {error}"))
}

/// `/health` is deliberately unauthenticated (and cheap), so readiness probes
/// do not need the per-process API key.
pub fn probe_health(addr: SocketAddr) -> bool {
    let Ok(response) = open_response(addr, None, "GET", "/health", None) else {
        return false;
    };
    if response.status != 200 {
        return false;
    }
    let mut text = String::new();
    let mut body = response.body;
    body.read_to_string(&mut text).is_ok()
}

fn truncate(text: &str) -> String {
    const LIMIT: usize = 4096;
    if text.len() <= LIMIT {
        return text.to_string();
    }
    let mut end = LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
