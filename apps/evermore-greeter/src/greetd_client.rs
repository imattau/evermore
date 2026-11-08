//! Speaks greetd's own IPC protocol directly (`greetd-ipc(7)`:
//! length-prefixed JSON over the socket named by `$GREETD_SOCK`) rather
//! than depending on a third-party crate for it -- the protocol is a
//! handful of message types over a length-prefixed frame, verified
//! against the real man page on the VM, not guessed. This is exactly
//! what a stock frontend (`agreety`, `tuigreet`) speaks; this binary
//! *is* a frontend, just one that happens to render with egui instead
//! of a terminal UI.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

pub struct GreetdClient {
    stream: UnixStream,
}

#[derive(Debug)]
pub enum AuthStep {
    /// Authentication succeeded; ready for `start_session`.
    Done,
    /// The user must answer this prompt. `secret` means the answer
    /// should be masked in the UI.
    Prompt { message: String, secret: bool },
    /// An informational or error message from PAM that doesn't need an
    /// answer -- acknowledge with `respond(None)` to continue.
    Notice { message: String, is_error: bool },
}

impl GreetdClient {
    pub fn connect() -> Result<Self, String> {
        let sock_path = std::env::var("GREETD_SOCK")
            .map_err(|_| "GREETD_SOCK is not set -- not running under greetd".to_string())?;
        let stream = UnixStream::connect(&sock_path)
            .map_err(|e| format!("connecting to {sock_path}: {e}"))?;
        Ok(Self { stream })
    }

    pub fn create_session(&mut self, username: &str) -> Result<AuthStep, String> {
        self.send(&serde_json::json!({"type": "create_session", "username": username}))?;
        self.read_auth_step()
    }

    /// Answers the most recent [`AuthStep::Prompt`] or acknowledges an
    /// [`AuthStep::Notice`] (pass `None`).
    pub fn respond(&mut self, response: Option<&str>) -> Result<AuthStep, String> {
        let msg = match response {
            Some(r) => serde_json::json!({"type": "post_auth_message_response", "response": r}),
            None => serde_json::json!({"type": "post_auth_message_response"}),
        };
        self.send(&msg)?;
        self.read_auth_step()
    }

    pub fn cancel_session(&mut self) {
        let _ = self.send(&serde_json::json!({"type": "cancel_session"}));
    }

    /// Starts `cmd` as the authenticated session and hands control back
    /// to greetd. Per `greetd-ipc(7)`, the session only actually starts
    /// once *this process* (the greeter) exits -- callers must exit
    /// promptly after this returns `Ok`, not linger.
    pub fn start_session(mut self, cmd: &[&str], env: &[&str]) -> Result<(), String> {
        self.send(&serde_json::json!({"type": "start_session", "cmd": cmd, "env": env}))?;
        match self.recv()?.get("type").and_then(|v| v.as_str()) {
            Some("success") => Ok(()),
            _ => Err("start_session was not acknowledged with success".to_string()),
        }
    }

    fn read_auth_step(&mut self) -> Result<AuthStep, String> {
        let resp = self.recv()?;
        match resp.get("type").and_then(|v| v.as_str()) {
            Some("success") => Ok(AuthStep::Done),
            Some("error") => {
                let description = resp
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("authentication error")
                    .to_string();
                Err(description)
            }
            Some("auth_message") => {
                let message = resp
                    .get("auth_message")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                match resp.get("auth_message_type").and_then(|v| v.as_str()) {
                    Some("secret") => Ok(AuthStep::Prompt {
                        message,
                        secret: true,
                    }),
                    Some("visible") => Ok(AuthStep::Prompt {
                        message,
                        secret: false,
                    }),
                    Some("error") => Ok(AuthStep::Notice {
                        message,
                        is_error: true,
                    }),
                    _ => Ok(AuthStep::Notice {
                        message,
                        is_error: false,
                    }),
                }
            }
            other => Err(format!("unexpected greetd response: {other:?}")),
        }
    }

    fn send(&mut self, msg: &serde_json::Value) -> Result<(), String> {
        let payload = msg.to_string();
        let len = (payload.len() as u32).to_ne_bytes();
        self.stream
            .write_all(&len)
            .and_then(|_| self.stream.write_all(payload.as_bytes()))
            .map_err(|e| format!("writing to greetd: {e}"))
    }

    fn recv(&mut self) -> Result<serde_json::Value, String> {
        let mut len_buf = [0u8; 4];
        self.stream
            .read_exact(&mut len_buf)
            .map_err(|e| format!("reading from greetd: {e}"))?;
        let len = u32::from_ne_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.stream
            .read_exact(&mut buf)
            .map_err(|e| format!("reading from greetd: {e}"))?;
        serde_json::from_slice(&buf).map_err(|e| format!("parsing greetd response: {e}"))
    }
}
