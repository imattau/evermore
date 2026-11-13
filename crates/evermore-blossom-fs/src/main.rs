mod blossom_client;
mod fuse_surface;
mod relay_client;
mod signer_client;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blossom_client::{BlossomClient, hex_sha256};
use nostr::prelude::*;
use relay_client::{KIND_FILE_INDEX, RelayClient};
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
    last_index: Mutex<Option<(u64, String)>>,
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
    let mountpoint = parse_arg("--mount").map(PathBuf::from);
    let cache_dir = parse_arg("--cache-dir").map(PathBuf::from);
    let relay_url = parse_arg("--relay").unwrap_or_else(|| DEFAULT_RELAY_URL.to_string());
    let label = parse_arg("--label").unwrap_or_else(|| "nostr".to_string());

    if let Some(source_url) = parse_arg("--mirror-url") {
        let signer = SignerClient::new(identity);
        let sha256 = parse_arg("--mirror-sha256")
            .or_else(|| sha256_from_blob_url(&source_url))
            .unwrap_or_else(|| fail("--mirror-url needs --mirror-sha256 when the source URL doesn't end with the blob hash"));
        let descriptor = BlossomClient::new(&blossom_url, &signer)
            .mirror(&source_url, &sha256)
            .unwrap_or_else(|e| fail(&e));
        println!(
            "{}",
            serde_json::to_string_pretty(&descriptor).unwrap_or_else(|e| fail(&e.to_string()))
        );
        return;
    }

    if let Some(path) = parse_arg("--publish-file") {
        publish_file(Path::new(&path), &blossom_url, SignerClient::new(identity));
        return;
    }

    let dir = if mountpoint.is_some() {
        cache_dir.unwrap_or_else(|| default_cache_dir(&label))
    } else {
        dir
    };

    fs::create_dir_all(&dir)
        .unwrap_or_else(|e| fail(&format!("couldn't create {}: {e}", dir.display())));

    let signer = SignerClient::new(identity);
    let own_pubkey = signer.get_public_key().unwrap_or_else(|e| {
        fail(&format!(
            "couldn't get public key from evermore-signer: {e}"
        ))
    });
    eprintln!("evermore-blossom-fs: identity pubkey {own_pubkey}");
    eprintln!(
        "evermore-blossom-fs: watching {} (label: {label})",
        dir.display()
    );

    let shared = Arc::new(Shared {
        dir,
        label,
        signer,
        blossom_url,
        own_pubkey: own_pubkey.clone(),
        manifest: Mutex::new(Manifest::new()),
        last_index: Mutex::new(None),
    });

    let relay = {
        let shared = Arc::clone(&shared);
        RelayClient::connect(&relay_url, own_pubkey, move |event| {
            on_index_event(&shared, event)
        })
        .unwrap_or_else(|e| fail(&format!("couldn't connect to relay at {relay_url}: {e}")))
    };
    eprintln!("evermore-blossom-fs: connected to relay at {relay_url}");

    let app = App { shared, relay };
    if let Some(mountpoint) = mountpoint {
        eprintln!("evermore-blossom-fs: mounting {}", mountpoint.display());
        if let Err(e) = fuse_surface::mount(app, &mountpoint) {
            fail(&e);
        }
    } else {
        watch_and_sync(app);
    }
}

fn sha256_from_blob_url(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let basename = path.rsplit('/').next()?;
    let digest = basename.split('.').next()?;
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}

fn publish_file(path: &Path, blossom_url: &str, signer: SignerClient) {
    let data =
        fs::read(path).unwrap_or_else(|e| fail(&format!("couldn't read {}: {e}", path.display())));
    let content_type = guess_content_type(path);
    let client = BlossomClient::new(blossom_url, &signer);
    let descriptor = client
        .upload(&data, content_type)
        .unwrap_or_else(|e| fail(&format!("couldn't upload {}: {e}", path.display())));
    let imeta = [
        "imeta".to_string(),
        format!("url {}", descriptor.url),
        format!("x {}", descriptor.sha256),
        format!("m {}", content_type),
        format!("size {}", descriptor.size),
    ];
    let imeta = imeta.join(" ");
    let copied = copy_to_clipboard(&format!("{imeta}\n"));
    if !copied {
        eprintln!("No Wayland or X11 clipboard tool found; reference follows:");
    }
    println!("{imeta}");
}

fn copy_to_clipboard(value: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (program, args) in [
        ("wl-copy", vec![]),
        ("xclip", vec!["-selection", "clipboard"]),
    ] {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(value.as_bytes()).is_ok());
        if wrote {
            // Clipboard providers often stay alive for as long as their data
            // remains selected. Don't block the file manager waiting for a
            // paste or selection change.
            return true;
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    false
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
    let Ok(mut remote): Result<Manifest, _> = serde_json::from_str(&event.content) else {
        eprintln!("evermore-blossom-fs: file index event wasn't valid JSON");
        return;
    };

    // Relays may replay stored replaceable events in a different order. Apply
    // only the newest event per directory (NIP-01 timestamp, then event-id
    // tie-break) and suppress our own event when it comes back over the socket.
    let stamp = event.created_at.as_secs();
    let event_id = event.id.to_hex();
    {
        let mut last = shared.last_index.lock().unwrap();
        if last
            .as_ref()
            .is_some_and(|(time, id)| *time > stamp || (*time == stamp && id <= &event_id))
        {
            return;
        }
        *last = Some((stamp, event_id));
    }

    // Directory indexes contain basenames only. Never let a relay event
    // turn into a path outside the configured sync directory.
    remote.retain(|name, entry| valid_filename(name) && valid_sha256(&entry.sha256));

    let client = BlossomClient::new(&shared.blossom_url, &shared.signer);
    let mut manifest = shared.manifest.lock().unwrap();
    let removed: Vec<_> = manifest
        .keys()
        .filter(|name| !remote.contains_key(*name))
        .cloned()
        .collect();
    for filename in removed {
        if let Some(previous) = manifest.remove(&filename) {
            let path = shared.dir.join(&filename);
            if fs::read(&path).is_ok_and(|bytes| hex_sha256(&bytes) == previous.sha256)
                && let Err(e) = fs::remove_file(&path)
            {
                eprintln!("evermore-blossom-fs: couldn't remove synced deletion {filename}: {e}");
            }
        }
    }
    for (filename, entry) in &remote {
        let local_path = shared.dir.join(filename);
        let local = fs::read(&local_path).ok();
        let needs_download = match &local {
            Some(bytes) => hex_sha256(bytes) != entry.sha256,
            None => true,
        };
        if needs_download {
            // Preserve an unindexed local edit. Marking the remote version in
            // the manifest lets the normal rescan detect and publish the
            // local bytes as the next version instead of discarding them.
            let local_is_clean = match local.as_ref() {
                Some(bytes) => manifest
                    .get(filename)
                    .is_some_and(|previous| hex_sha256(bytes) == previous.sha256),
                // A formerly synced file missing locally is a pending local
                // deletion; don't resurrect it while applying another device's
                // index update. An unseen remote file can still be downloaded.
                None => !manifest.contains_key(filename),
            };
            if !local_is_clean {
                if local.is_some() {
                    eprintln!(
                        "evermore-blossom-fs: preserving local edit for {filename}; remote update was not written"
                    );
                } else {
                    eprintln!(
                        "evermore-blossom-fs: preserving local deletion for {filename}; remote file was not restored"
                    );
                }
                manifest.insert(filename.clone(), entry.clone());
                continue;
            }
            match client.download(&entry.sha256) {
                Ok(bytes)
                    if hex_sha256(&bytes) == entry.sha256 && bytes.len() as u64 == entry.size =>
                {
                    if let Err(e) = fs::write(&local_path, bytes) {
                        eprintln!("evermore-blossom-fs: couldn't write {filename}: {e}");
                        continue;
                    }
                    eprintln!("evermore-blossom-fs: downloaded {filename} from the relay's index");
                }
                Ok(_) => {
                    eprintln!(
                        "evermore-blossom-fs: downloaded {filename} failed its SHA-256 check"
                    );
                    continue;
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
            eprintln!(
                "evermore-blossom-fs: couldn't read {}: {e}",
                shared.dir.display()
            );
            return;
        }
    };

    let mut changed = false;
    let mut seen = std::collections::HashSet::new();
    let client = BlossomClient::new(&shared.blossom_url, &shared.signer);
    let mut manifest = shared.manifest.lock().unwrap();

    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let Some(filename) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !valid_filename(&filename) {
            continue;
        }
        seen.insert(filename.clone());
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

    let deleted: Vec<_> = manifest
        .keys()
        .filter(|name| !seen.contains(*name))
        .cloned()
        .collect();
    for name in deleted {
        manifest.remove(&name);
        changed = true;
        eprintln!("evermore-blossom-fs: removed {name} from the shared file index");
    }

    if changed {
        publish_index(app, &manifest);
    }
}

fn valid_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && Path::new(name).components().count() == 1
}

fn valid_sha256(sha256: &str) -> bool {
    sha256.len() == 64
        && sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
    let mut last = shared.last_index.lock().unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let created_at = match last.as_ref() {
        Some((previous, _)) if now <= *previous => *previous + 1,
        _ => now,
    };
    if created_at > now {
        std::thread::sleep(Duration::from_secs(created_at - now));
    }
    match shared.signer.sign_event_at(
        KIND_FILE_INDEX,
        content,
        vec![vec!["d".to_string(), d_tag]],
        Some(created_at),
    ) {
        Ok(event) => {
            *last = Some((created_at, event.id.to_hex()));
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

fn default_cache_dir(label: &str) -> PathBuf {
    let cache = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
                .join(".cache")
        });
    cache.join("evermore").join("blossom").join(label)
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
