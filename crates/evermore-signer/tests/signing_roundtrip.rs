//! M1.6 validation: a client connects to the daemon's socket, asks it to
//! sign a note, and verifies the signature cryptographically -- proving the
//! daemon actually produces valid Nostr events without the client ever
//! touching the secret key. This test drives the real `evermore-signer`
//! binary as a subprocess against a throwaway identity and socket, so it
//! doesn't disturb a real session's keyring or socket.
//!
//! Requires a Linux kernel keyring (works in the build VM and on a normal
//! Linux host; skipped-by-failure in restricted sandboxes without one).
//!
//! Note: identity storage is the OS *session* keyring, not namespaced by
//! `XDG_RUNTIME_DIR`, so this intentionally leaves a `test-roundtrip` key
//! behind in whatever session runs the test -- harmless (same identity name
//! is reused/overwritten on every run), just not auto-cleaned.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command};
use std::time::Duration;

use nostr::prelude::*;

struct DaemonGuard(Child);
impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bin_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // test binary's own dir
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("evermore-signer")
}

fn send(stream: &mut UnixStream, req: &serde_json::Value) -> serde_json::Value {
    let mut line = req.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut response_line = String::new();
    reader.read_line(&mut response_line).unwrap();
    serde_json::from_str(&response_line).unwrap()
}

#[test]
fn sign_event_roundtrip_produces_a_valid_signature() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let identity = "test-roundtrip";

    // `import` (not `keygen`) so reruns are idempotent -- keygen refuses to
    // overwrite an existing identity, which `test-roundtrip` always is after
    // the first run (see the module doc comment on keyring scoping).
    let throwaway = Keys::generate();
    let mut import = Command::new(bin_path())
        .arg("import")
        .arg(identity)
        .env("XDG_RUNTIME_DIR", runtime_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run `evermore-signer import` -- is it built?");
    import
        .stdin
        .take()
        .unwrap()
        .write_all(throwaway.secret_key().to_secret_hex().as_bytes())
        .unwrap();
    let import = import.wait_with_output().unwrap();
    assert!(
        import.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&import.stderr)
    );

    let daemon = DaemonGuard(
        Command::new(bin_path())
            .arg("serve")
            .env("XDG_RUNTIME_DIR", runtime_dir.path())
            .spawn()
            .expect("failed to spawn evermore-signer serve"),
    );

    let socket_path = runtime_dir.path().join("evermore/signer.sock");
    let mut stream = None;
    for _ in 0..50 {
        if let Ok(s) = UnixStream::connect(&socket_path) {
            stream = Some(s);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut stream = stream.expect("daemon never created its socket");

    let pk_resp = send(
        &mut stream,
        &serde_json::json!({"method": "get_public_key", "params": {"identity": identity}}),
    );
    assert_eq!(pk_resp["status"], "ok");
    let pubkey_hex = pk_resp["data"]["pubkey_hex"].as_str().unwrap();

    let sign_resp = send(
        &mut stream,
        &serde_json::json!({
            "method": "sign_event",
            "params": {
                "identity": identity,
                "kind": 1,
                "content": "hello from evermore-signer's own test suite",
                "tags": []
            }
        }),
    );
    assert_eq!(
        sign_resp["status"], "ok",
        "sign_event failed: {sign_resp:?}"
    );

    // The daemon never handed us the secret key -- only a signed event.
    // Parse it back into a real `Event` and verify the signature
    // cryptographically, against the pubkey the daemon separately reported.
    let event: Event = serde_json::from_value(sign_resp["data"].clone()).unwrap();
    assert_eq!(event.pubkey.to_hex(), pubkey_hex);
    event
        .verify()
        .expect("daemon produced an event with an invalid signature");

    drop(daemon);
}
