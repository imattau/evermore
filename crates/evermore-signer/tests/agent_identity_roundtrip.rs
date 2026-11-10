//! M11.7 validation: the signer can create a persisted agent identity over
//! its socket, reload persisted identities at startup, and delete both the
//! keyring entry and the on-disk secret.
//!
//! Like `signing_roundtrip.rs`, this drives the real binary as a subprocess.
//! It uses a unique slug per run because the kernel session keyring is not
//! namespaced by `XDG_RUNTIME_DIR`; any identity left behind by a failed run
//! is harmless but should be cleaned up by the passing path.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
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
    path.pop();
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

fn start_daemon(runtime_dir: &Path, data_dir: &Path) -> DaemonGuard {
    DaemonGuard(
        Command::new(bin_path())
            .arg("serve")
            .env("XDG_RUNTIME_DIR", runtime_dir)
            .env("XDG_DATA_HOME", data_dir)
            .spawn()
            .expect("failed to spawn evermore-signer serve"),
    )
}

fn connect(runtime_dir: &Path) -> UnixStream {
    let socket_path = runtime_dir.join("evermore/signer.sock");
    for _ in 0..50 {
        if let Ok(stream) = UnixStream::connect(&socket_path) {
            return stream;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "daemon never created its socket at {}",
        socket_path.display()
    );
}

fn unique_slug(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{prefix}-{nanos}")
}

fn identity_file(data_dir: &Path, name: &str) -> std::path::PathBuf {
    data_dir
        .join("evermore")
        .join("identities")
        .join(format!("{name}.nsec"))
}

#[test]
fn create_and_delete_agent_identity_persists_the_secret() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let slug = unique_slug("create-delete");
    let name = format!("agent-{slug}");

    let daemon = start_daemon(runtime_dir.path(), data_dir.path());
    let mut stream = connect(runtime_dir.path());

    let created = send(
        &mut stream,
        &serde_json::json!({
            "method": "create_agent_identity",
            "params": { "slug": slug }
        }),
    );
    assert_eq!(created["status"], "ok", "create failed: {created:?}");
    assert_eq!(created["data"]["identity"], name);
    assert_eq!(created["data"]["persisted"], true);
    let pubkey_hex = created["data"]["pubkey_hex"].as_str().unwrap().to_string();

    let path = identity_file(data_dir.path(), &name);
    assert!(
        path.is_file(),
        "expected persisted secret at {}",
        path.display()
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let listed = send(
        &mut stream,
        &serde_json::json!({"method": "list_identities"}),
    );
    assert!(
        listed["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == &serde_json::Value::String(name.clone()))
    );

    let pk = send(
        &mut stream,
        &serde_json::json!({
            "method": "get_public_key",
            "params": { "identity": name }
        }),
    );
    assert_eq!(pk["status"], "ok");
    assert_eq!(pk["data"]["pubkey_hex"], pubkey_hex);

    drop(daemon);

    // A fresh daemon with the same data dir must reload the identity.
    let daemon = start_daemon(runtime_dir.path(), data_dir.path());
    let mut stream = connect(runtime_dir.path());
    let listed = send(
        &mut stream,
        &serde_json::json!({"method": "list_identities"}),
    );
    assert!(
        listed["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == &serde_json::Value::String(name.clone())),
        "persisted identity was not reloaded at startup: {listed:?}"
    );

    let deleted = send(
        &mut stream,
        &serde_json::json!({
            "method": "delete_agent_identity",
            "params": { "slug": slug }
        }),
    );
    assert_eq!(deleted["status"], "ok", "delete failed: {deleted:?}");
    assert_eq!(deleted["data"]["persisted_removed"], true);
    assert!(!path.exists(), "secret file survived deletion");

    let pk = send(
        &mut stream,
        &serde_json::json!({
            "method": "get_public_key",
            "params": { "identity": name }
        }),
    );
    assert_eq!(pk["status"], "err", "deleted identity still resolves");

    let unknown = send(
        &mut stream,
        &serde_json::json!({
            "method": "delete_agent_identity",
            "params": { "slug": slug }
        }),
    );
    assert_eq!(unknown["status"], "err");

    drop(daemon);
}

#[test]
fn persisted_identity_is_loaded_without_a_prior_keygen() {
    let runtime_dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let slug = unique_slug("reload-only");
    let name = format!("agent-{slug}");

    let identities_dir = data_dir.path().join("evermore").join("identities");
    std::fs::create_dir_all(&identities_dir).unwrap();
    std::fs::set_permissions(&identities_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = identities_dir.join(format!("{name}.nsec"));
    std::fs::write(&path, Keys::generate().secret_key().to_secret_hex()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let daemon = start_daemon(runtime_dir.path(), data_dir.path());
    let mut stream = connect(runtime_dir.path());

    let pk = send(
        &mut stream,
        &serde_json::json!({
            "method": "get_public_key",
            "params": { "identity": name }
        }),
    );
    assert_eq!(
        pk["status"], "ok",
        "persisted identity was not loaded: {pk:?}"
    );

    let deleted = send(
        &mut stream,
        &serde_json::json!({
            "method": "delete_agent_identity",
            "params": { "slug": slug }
        }),
    );
    assert_eq!(deleted["status"], "ok");
    assert!(!path.exists());

    drop(daemon);
}

#[test]
fn ensure_service_identities_is_idempotent() {
    let data_dir = tempfile::tempdir().unwrap();
    let owner = unique_slug("svc");
    let operations = identity_file(data_dir.path(), &format!("{owner}-operations"));
    let assistant = identity_file(data_dir.path(), &format!("{owner}-assistant"));

    let run = || {
        Command::new(bin_path())
            .args(["ensure-service-identities", &owner])
            .env("XDG_DATA_HOME", data_dir.path())
            .output()
            .expect("failed to run ensure-service-identities")
    };

    let first = run();
    assert!(
        first.status.success(),
        "ensure failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_output = String::from_utf8_lossy(&first.stdout).to_string();
    assert!(first_output.contains(&format!("{owner}-operations: ")));
    assert!(first_output.contains(&format!("{owner}-assistant: ")));
    assert!(operations.is_file());
    assert!(assistant.is_file());
    assert_eq!(
        std::fs::metadata(&operations).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let first_operations = std::fs::read(&operations).unwrap();
    let first_assistant = std::fs::read(&assistant).unwrap();

    let second = run();
    assert!(
        second.status.success(),
        "second ensure failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(std::fs::read(&operations).unwrap(), first_operations);
    assert_eq!(std::fs::read(&assistant).unwrap(), first_assistant);
}
