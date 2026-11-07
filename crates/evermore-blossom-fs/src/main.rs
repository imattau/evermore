mod blossom_client;
mod relay_client;
mod signer_client;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use blossom_client::{hex_sha256, BlossomClient};
use nostr::prelude::*;
use relay_client::{RelayClient, KIND_FILE_INDEX};
use serde::{Deserialize, Serialize};
use signer_client::SignerClient;

const DEFAULT_RELAY_URL: &str = "ws://127.0.0.1:4869";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
struct ManifestEntry {
    sha256: String,
    size: u64,
}

type Manifest = HashMap<String, ManifestEntry>;

/// Shared state the relay callback (on its own thread) and the file
/// watcher loop (on main) both touch -- built first, so the relay
/// connection's callback can close over clones of it without needing to
/// know about the `RelayClient` it's itself producing.
struct Shared {
    dir: PathBuf,
    label: String,
    signer: SignerClient,
    blossom_url: String,
    own_pubkey: String,
    manifest: Mutex<Manifest>,
}

struct App {
    shared: Arc<Shared>,
    relay: RelayClient,
}

fn main() {
    let identity = match parse_arg("--identity") {
        Some(v) => v,
        None => fail("missing required --identity NAME"),
    };
    let blossom_url = match parse_arg("--blossom-url") {
        Some(v) => v,
        None => fail("missing required --blossom-url URL"),
    };
    let dir = parse_arg("--dir")
        .map(PathBuf::from)
        .unwrap_or_else(default_dir);
    let relay_url = parse_arg("--relay").unwrap_or_else(|| DEFAULT_RELAY_URL.to_string());
    let label = parse_arg("--label").unwrap_or_else(|| "nostr".to_string());

    fs::create_dir_all(&dir).unwrap_or_else(|e| fail(&format!("couldn't create {}: {e}", dir.display())));

    let signer = SignerClient::new(identity);
    let own_pubkey = signer
        .get_public_key()
        .unwrap_or_else(|e| fail(&format!("couldn't get public key from evermore-signer: {e}")));
    eprintln!("evermore-blossom-fs: identity pubkey {own_pubkey}");
    eprintln!("evermore-blossom-fs: watching {} (label: {label})", dir.display());

    let shared = Arc::new(Shared {
        dir,
        label,
        signer,
        blossom_url,
        own_pubkey: own_pubkey.clone(),
        manifest: Mutex::new(Manifest::new()),
    });

    let relay = {
        let shared = Arc::clone(&shared);
        RelayClient::connect(&relay_url, own_pubkey, move |event| on_index_event(&shared, event))
            .unwrap_or_else(|e| fail(&format!("couldn't connect to relay at {relay_url}: {e}")))
    };
    eprintln!("evermore-blossom-fs: connected to relay at {relay_url}");

    watch_and_sync(App { shared, relay });
}

/// Handles an incoming file-index event from the relay: anything in it we
/// don't have locally (or have with a different hash) gets downloaded.
fn on_index_event(shared: &Shared, event: Event) {
    if event.kind != Kind::from(KIND_FILE_INDEX) || event.pubkey.to_hex() != shared.own_pubkey {
        return;
    }
    let Some(d) = event.tags.iter().find_map(|t| {
        let s = t.as_slice();
        (s.first().map(String::as_str) == Some("d")).then(|| s.get(1).cloned())?
    }) else {
        return;
    };
    if d != format!("evermore:files:{}", shared.label) {
        return;
    }
    let Ok(remote): Result<Manifest, _> = serde_json::from_str(&event.content) else {
        eprintln!("evermore-blossom-fs: file index event wasn't valid JSON");
        return;
    };

    let client = BlossomClient::new(&shared.blossom_url, &shared.signer);
    let mut manifest = shared.manifest.lock().unwrap();
    for (filename, entry) in &remote {
        let local_path = shared.dir.join(filename);
        let needs_download = match fs::read(&local_path) {
            Ok(bytes) => hex_sha256(&bytes) != entry.sha256,
            Err(_) => true,
        };
        if needs_download {
            match client.download(&entry.sha256) {
                Ok(bytes) => {
                    if let Err(e) = fs::write(&local_path, bytes) {
                        eprintln!("evermore-blossom-fs: couldn't write {filename}: {e}");
                        continue;
                    }
                    eprintln!("evermore-blossom-fs: downloaded {filename} from the relay's index");
                }
                Err(e) => {
                    eprintln!("evermore-blossom-fs: couldn't download {filename}: {e}");
                    continue;
                }
            }
        }
        manifest.insert(filename.clone(), entry.clone());
    }
}

/// Watches `app.shared.dir` and, on any change, rescans it: anything new or
/// whose hash no longer matches the in-memory manifest gets uploaded and
/// the index event republished. A file we *just wrote* from
/// `on_index_event` will already match the manifest by the time this
/// fires, so it's a natural no-op -- no separate "ignore the next event"
/// bookkeeping needed.
fn watch_and_sync(app: App) {
    use notify::{RecursiveMode, Watcher};
    use std::sync::mpsc::channel;

    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .unwrap_or_else(|e| fail(&format!("couldn't create file watcher: {e}")));
    watcher
        .watch(&app.shared.dir, RecursiveMode::NonRecursive)
        .unwrap_or_else(|e| fail(&format!("couldn't watch {}: {e}", app.shared.dir.display())));

    rescan(&app); // pick up anything already in the directory on startup

    // Debounced: a single `echo foo > file` produces multiple inotify
    // events at different points in the write (file created at 0 bytes,
    // then a modify event once content lands) -- rescanning on each one
    // independently raced uploads against partially-written files and let
    // a stale/short read clobber a correct one that arrived later. Instead,
    // drain every event that shows up within DEBOUNCE of the last one
    // before rescanning, so we only ever read files that have gone quiet.
    const DEBOUNCE: Duration = Duration::from_millis(300);
    loop {
        if rx.recv().is_err() {
            break; // watcher's sender dropped
        }
        while rx.recv_timeout(DEBOUNCE).is_ok() {}
        rescan(&app);
    }
}

fn rescan(app: &App) {
    let shared = &app.shared;
    let entries = match fs::read_dir(&shared.dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("evermore-blossom-fs: couldn't read {}: {e}", shared.dir.display());
            return;
        }
    };

    let mut changed = false;
    let client = BlossomClient::new(&shared.blossom_url, &shared.signer);
    let mut manifest = shared.manifest.lock().unwrap();

    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let Some(filename) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        let sha256 = hex_sha256(&bytes);
        if manifest.get(&filename).map(|e| &e.sha256) == Some(&sha256) {
            continue; // unchanged (or just downloaded -- see watch_and_sync's doc comment)
        }

        match client.upload(&bytes, guess_content_type(&entry.path())) {
            Ok(desc) => {
                manifest.insert(
                    filename.clone(),
                    ManifestEntry {
                        sha256,
                        size: desc.size,
                    },
                );
                eprintln!("evermore-blossom-fs: uploaded {filename}");
                changed = true;
            }
            Err(e) => eprintln!("evermore-blossom-fs: couldn't upload {filename}: {e}"),
        }
    }

    if changed {
        publish_index(app, &manifest);
    }
}

fn publish_index(app: &App, manifest: &Manifest) {
    let shared = &app.shared;
    let content = match serde_json::to_string(manifest) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("evermore-blossom-fs: couldn't serialize index: {e}");
            return;
        }
    };
    let d_tag = format!("evermore:files:{}", shared.label);
    match shared
        .signer
        .sign_event(KIND_FILE_INDEX, content, vec![vec!["d".to_string(), d_tag]])
    {
        Ok(event) => {
            if let Err(e) = app.relay.publish(&event) {
                eprintln!("evermore-blossom-fs: couldn't publish index: {e}");
            }
        }
        Err(e) => eprintln!("evermore-blossom-fs: couldn't sign index event: {e}"),
    }
}

fn guess_content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("txt" | "md") => "text/plain",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => "application/octet-stream",
    }
}

fn default_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join("Nostr")
}

fn parse_arg(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}
