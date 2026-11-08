//! evermore-npack: a stateless CLI, not a daemon. Unlike settings/
//! notifications/operations, package discovery and install don't need a
//! live subscription or cached state shared across callers -- each
//! command queries the relay fresh (NIP-77 negentropy or a local cache
//! would speed this up later, but a plain REQ/EOSE round trip is fine for
//! a first version and keeps this crate simple). "Installed" state is
//! just the filesystem under install_root() -- no separate database.

use evermore_npack::{
    Manifest, PackageListing, ReleaseListing, archive, bin_dir, blossom_client, install_root,
    relay_client, signer_client::SignerClient,
};
use nostr::prelude::*;
use std::path::PathBuf;

const DEFAULT_RELAY_URL: &str = "ws://127.0.0.1:4869";

fn main() {
    let result = run();
    if let Err(e) = result {
        eprintln!("evermore-npack: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("trust") => trust_cmd(args.next(), true),
        Some("untrust") => trust_cmd(args.next(), false),
        Some("trusted") => trusted_cmd(),
        Some("publish") => publish_cmd(args.next(), args.next()),
        Some("available") => available_cmd(),
        Some("install") => install_cmd(args.next()),
        Some("remove") => remove_cmd(args.next()),
        Some("installed") => installed_cmd(),
        _ => {
            eprintln!(
                "usage: evermore-npack <trust|untrust|trusted|publish|available|install|remove|installed>"
            );
            eprintln!("  trust PUBKEY_HEX --identity NAME [--relay URL]");
            eprintln!("  untrust PUBKEY_HEX --identity NAME [--relay URL]");
            eprintln!("  trusted --identity NAME [--relay URL]");
            eprintln!(
                "  publish MANIFEST.json PAYLOAD_DIR --identity NAME --blossom-url URL [--relay URL]"
            );
            eprintln!("  available --identity NAME [--relay URL]");
            eprintln!("  install PACKAGE_ID --identity NAME --blossom-url URL [--relay URL]");
            eprintln!("  remove PACKAGE_ID");
            eprintln!("  installed");
            std::process::exit(2);
        }
    }
}

fn relay_arg() -> String {
    arg_value("--relay").unwrap_or_else(|| DEFAULT_RELAY_URL.to_string())
}

fn identity_arg() -> Result<SignerClient, String> {
    let identity = arg_value("--identity").ok_or("missing --identity NAME")?;
    Ok(SignerClient::new(identity))
}

fn require(value: Option<String>, what: &str) -> Result<String, String> {
    value.ok_or_else(|| format!("missing {what}"))
}

// --- trust list (NIP-51 kind:30000, d=evermore:trusted-publishers) ---

fn trust_cmd(pubkey: Option<String>, add: bool) -> Result<(), String> {
    let pubkey = require(pubkey, "PUBKEY_HEX")?;
    PublicKey::parse(&pubkey).map_err(|e| format!("invalid pubkey: {e}"))?;
    let signer = identity_arg()?;
    let relay = relay_arg();
    let own_pubkey = signer.get_public_key()?;

    let mut list = fetch_trust_list(&relay, &own_pubkey)?;
    if add {
        if !list.contains(&pubkey) {
            list.push(pubkey);
        }
    } else {
        list.retain(|p| p != &pubkey);
    }

    let tags: Vec<Vec<String>> = std::iter::once(vec![
        "d".to_string(),
        evermore_npack::TRUSTED_PUBLISHERS_D_TAG.to_string(),
    ])
    .chain(list.iter().map(|p| vec!["p".to_string(), p.clone()]))
    .collect();
    let event = signer.sign_event(evermore_npack::KIND_TRUSTED_PUBLISHERS, String::new(), tags)?;
    relay_client::publish(&relay, &event)?;
    println!("trust list now has {} publisher(s)", list.len());
    Ok(())
}

fn trusted_cmd() -> Result<(), String> {
    let signer = identity_arg()?;
    let relay = relay_arg();
    let own_pubkey = signer.get_public_key()?;
    let list = fetch_trust_list(&relay, &own_pubkey)?;
    if json_flag() {
        println!(
            "{}",
            serde_json::to_string(&list).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if list.is_empty() {
        println!("(no trusted publishers yet -- `evermore-npack trust PUBKEY_HEX`)");
    }
    for pubkey in list {
        println!("{pubkey}");
    }
    Ok(())
}

fn fetch_trust_list(relay: &str, own_pubkey: &str) -> Result<Vec<String>, String> {
    let filter = serde_json::json!({
        "kinds": [evermore_npack::KIND_TRUSTED_PUBLISHERS],
        "authors": [own_pubkey],
        "#d": [evermore_npack::TRUSTED_PUBLISHERS_D_TAG],
    });
    let events = relay_client::query(relay, filter)?;
    let latest = events.into_iter().max_by_key(|e| e.created_at);
    Ok(latest
        .map(|e| {
            e.tags
                .iter()
                .filter(|t| t.as_slice().first().map(String::as_str) == Some("p"))
                .filter_map(|t| t.as_slice().get(1).cloned())
                .collect()
        })
        .unwrap_or_default())
}

// --- publish ---

fn publish_cmd(manifest_path: Option<String>, payload_dir: Option<String>) -> Result<(), String> {
    let manifest_path = PathBuf::from(require(manifest_path, "MANIFEST.json")?);
    let payload_dir = PathBuf::from(require(payload_dir, "PAYLOAD_DIR")?);
    let blossom_url = arg_value("--blossom-url").ok_or("missing --blossom-url URL")?;
    let signer = identity_arg()?;
    let relay = relay_arg();

    let manifest_json = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("reading {}: {e}", manifest_path.display()))?;
    let manifest: Manifest =
        serde_json::from_str(&manifest_json).map_err(|e| format!("invalid manifest: {e}"))?;

    let archive_bytes = archive::build(&manifest, &payload_dir)?;
    let blob_url = blossom_client::upload(&blossom_url, &signer, &archive_bytes)?;
    let sha256 = blossom_client::hex_sha256(&archive_bytes);

    let app_event = signer.sign_event(
        evermore_npack::KIND_SOFTWARE_APPLICATION,
        manifest.description.clone(),
        vec![
            vec!["d".to_string(), manifest.id.clone()],
            vec!["name".to_string(), manifest.name.clone()],
        ],
    )?;
    relay_client::publish(&relay, &app_event)?;

    let release_event = signer.sign_event(
        evermore_npack::KIND_RELEASE_ARTIFACT_SET,
        String::new(),
        vec![
            vec!["d".to_string(), manifest.id.clone()],
            vec!["version".to_string(), manifest.version.clone()],
            vec!["url".to_string(), blob_url.clone()],
            vec!["x".to_string(), sha256.clone()],
        ],
    )?;
    relay_client::publish(&relay, &release_event)?;

    println!(
        "published {} {} -> {blob_url} ({sha256})",
        manifest.id, manifest.version
    );
    Ok(())
}

// --- discovery ---

fn available_cmd() -> Result<(), String> {
    let signer = identity_arg()?;
    let relay = relay_arg();
    let own_pubkey = signer.get_public_key()?;
    let trust_list = fetch_trust_list(&relay, &own_pubkey)?;
    if trust_list.is_empty() {
        if json_flag() {
            println!("[]");
        } else {
            println!("(no trusted publishers -- `evermore-npack trust PUBKEY_HEX`)");
        }
        return Ok(());
    }

    let listings = discover_packages(&relay, &trust_list)?;
    if json_flag() {
        let combined: Vec<serde_json::Value> = listings
            .into_iter()
            .map(|listing| {
                let release = fetch_latest_release(&relay, &listing.id, &trust_list)
                    .ok()
                    .flatten();
                serde_json::json!({
                    "id": listing.id,
                    "name": listing.name,
                    "description": listing.description,
                    "publisher_pubkey": listing.publisher_pubkey,
                    "version": release.as_ref().map(|r| r.version.clone()),
                    "sha256": release.map(|r| r.sha256),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string(&combined).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if listings.is_empty() {
        println!("(no packages found from trusted publishers)");
    }
    for listing in listings {
        let release = fetch_latest_release(&relay, &listing.id, &trust_list)?;
        let version = release
            .map(|r| r.version)
            .unwrap_or_else(|| "?".to_string());
        println!(
            "{}  {} v{}  -- {} (by {})",
            listing.id,
            listing.name,
            version,
            listing.description,
            short(&listing.publisher_pubkey)
        );
    }
    Ok(())
}

fn discover_packages(relay: &str, trust_list: &[String]) -> Result<Vec<PackageListing>, String> {
    let filter = serde_json::json!({
        "kinds": [evermore_npack::KIND_SOFTWARE_APPLICATION],
        "authors": trust_list,
    });
    let events = relay_client::query(relay, filter)?;
    let mut by_id: std::collections::HashMap<String, Event> = std::collections::HashMap::new();
    for event in events {
        let Some(id) = d_tag(&event) else { continue };
        let newer = by_id
            .get(&id)
            .is_none_or(|existing| event.created_at > existing.created_at);
        if newer {
            by_id.insert(id, event);
        }
    }
    Ok(by_id
        .into_values()
        .map(|e| {
            let name = tag_value(&e, "name").unwrap_or_else(|| d_tag(&e).unwrap_or_default());
            PackageListing {
                id: d_tag(&e).unwrap_or_default(),
                name,
                description: e.content.clone(),
                publisher_pubkey: e.pubkey.to_hex(),
            }
        })
        .collect())
}

fn fetch_latest_release(
    relay: &str,
    id: &str,
    trust_list: &[String],
) -> Result<Option<ReleaseListing>, String> {
    Ok(fetch_latest_release_event(relay, id, trust_list)?.map(|(release, _)| release))
}

fn fetch_latest_release_event(
    relay: &str,
    id: &str,
    trust_list: &[String],
) -> Result<Option<(ReleaseListing, serde_json::Value)>, String> {
    let filter = serde_json::json!({
        "kinds": [evermore_npack::KIND_RELEASE_ARTIFACT_SET],
        "authors": trust_list,
        "#d": [id],
    });
    let events = relay_client::query(relay, filter)?;
    let latest = events.into_iter().max_by_key(|e| e.created_at);
    Ok(latest.and_then(|event| {
        let release = ReleaseListing {
            id: id.to_string(),
            version: tag_value(&event, "version")?,
            publisher_pubkey: event.pubkey.to_hex(),
            blossom_url: tag_value(&event, "url")?,
            sha256: tag_value(&event, "x")?,
        };
        let signed_event = serde_json::to_value(event).ok()?;
        Some((release, signed_event))
    }))
}

// --- install / remove / installed ---

fn install_cmd(id: Option<String>) -> Result<(), String> {
    let id = require(id, "PACKAGE_ID")?;
    if !safe_package_component(&id) {
        return Err("PACKAGE_ID contains characters unsafe for an install path".into());
    }
    let blossom_url = arg_value("--blossom-url").ok_or("missing --blossom-url URL")?;
    let signer = identity_arg()?;
    let relay = relay_arg();
    let own_pubkey = signer.get_public_key()?;

    let trust_list = fetch_trust_list(&relay, &own_pubkey)?;
    if trust_list.is_empty() {
        return Err("no trusted publishers -- `evermore-npack trust PUBKEY_HEX` first".into());
    }
    let requested_version = arg_value("--version");
    let requested_sha256 = arg_value("--sha256");
    if requested_version.is_some() != requested_sha256.is_some() {
        return Err("--version and --sha256 must be supplied together".into());
    }
    let (release, signed_release_event) = if let (Some(version), Some(sha256)) =
        (&requested_version, &requested_sha256)
    {
        if !safe_package_component(version)
            || sha256.len() != 64
            || !sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("pinned version or SHA-256 is malformed".into());
        }
        let events = relay_client::query(
            &relay,
            serde_json::json!({
                "kinds": [evermore_npack::KIND_RELEASE_ARTIFACT_SET], "authors": trust_list, "#d": [&id]
            }),
        )?;
        events
            .into_iter()
            .find_map(|event| {
                let candidate = ReleaseListing {
                    id: id.clone(),
                    version: tag_value(&event, "version")?,
                    publisher_pubkey: event.pubkey.to_hex(),
                    blossom_url: tag_value(&event, "url")?,
                    sha256: tag_value(&event, "x")?,
                };
                if candidate.version == *version && candidate.sha256.eq_ignore_ascii_case(sha256) {
                    Some((candidate, serde_json::to_value(event).ok()?))
                } else {
                    None
                }
            })
            .ok_or_else(|| format!("no trusted signed release matches {id} {version} {sha256}"))?
    } else {
        fetch_latest_release_event(&relay, &id, &trust_list)?
            .ok_or_else(|| format!("no release of '{id}' found from a trusted publisher"))?
    };

    if !safe_package_component(&release.version)
        || release.sha256.len() != 64
        || !release.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("trusted release contains an unsafe version or malformed SHA-256".into());
    }

    let archive_bytes = blossom_client::download(&blossom_url, &release.sha256)
        .or_else(|_| blossom_client::download(&release.blossom_url, &release.sha256))?;
    let actual_sha256 = blossom_client::hex_sha256(&archive_bytes);
    if !actual_sha256.eq_ignore_ascii_case(&release.sha256) {
        return Err(format!(
            "downloaded content doesn't match the signed release (expected {}, got {actual_sha256}) -- refusing to install",
            release.sha256
        ));
    }

    let version_dir = install_root().join(&id).join(&release.version);
    let manifest = archive::extract(&archive_bytes, &version_dir)?;
    if manifest.id != id {
        return Err(format!(
            "archive manifest id '{}' doesn't match requested package '{id}'",
            manifest.id
        ));
    }
    if manifest.version != release.version {
        return Err(format!(
            "archive manifest version '{}' doesn't match signed release '{}'",
            manifest.version, release.version
        ));
    }

    std::fs::write(
        version_dir.join("evermore-release-event.json"),
        serde_json::to_vec(&signed_release_event).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("writing signed release evidence: {e}"))?;
    std::fs::write(version_dir.join("package.npk"), &archive_bytes)
        .map_err(|e| format!("retaining verified package archive: {e}"))?;

    let current_link = install_root().join(&id).join("current");
    let _ = std::fs::remove_file(&current_link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&version_dir, &current_link)
        .map_err(|e| format!("linking current version: {e}"))?;

    if let Some(bin) = &manifest.bin {
        let target = version_dir.join(bin);
        let link_name = bin_dir().join(bin.rsplit('/').next().unwrap_or(bin));
        std::fs::create_dir_all(bin_dir()).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&link_name);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, &link_name)
                .map_err(|e| format!("linking {bin}: {e}"))?;
            let mut perms = std::fs::metadata(&target)
                .map_err(|e| e.to_string())?
                .permissions();
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(perms.mode() | 0o111);
            std::fs::set_permissions(&target, perms).map_err(|e| e.to_string())?;
        }
        println!("linked {} -> {}", link_name.display(), target.display());
    }

    println!(
        "installed {} {} from {}",
        manifest.id,
        manifest.version,
        short(&release.publisher_pubkey)
    );
    Ok(())
}

fn safe_package_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

fn remove_cmd(id: Option<String>) -> Result<(), String> {
    let id = require(id, "PACKAGE_ID")?;
    if !safe_package_component(&id) {
        return Err("PACKAGE_ID contains characters unsafe for an install path".into());
    }
    let package_dir = install_root().join(&id);
    let current_link = package_dir.join("current");

    if let Ok(target) = std::fs::read_link(&current_link)
        && let Ok(manifest_json) = std::fs::read_to_string(target.join("manifest.json"))
        && let Ok(manifest) = serde_json::from_str::<Manifest>(&manifest_json)
        && let Some(bin) = manifest.bin
    {
        let link_name = bin_dir().join(bin.rsplit('/').next().unwrap_or(&bin));
        let _ = std::fs::remove_file(link_name);
    }

    std::fs::remove_dir_all(&package_dir)
        .map_err(|e| format!("removing {}: {e}", package_dir.display()))?;
    println!("removed {id}");
    Ok(())
}

fn installed_cmd() -> Result<(), String> {
    let root = install_root();
    let manifests = if root.is_dir() {
        collect_installed(&root)?
    } else {
        Vec::new()
    };

    if json_flag() {
        println!(
            "{}",
            serde_json::to_string(&manifests).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if manifests.is_empty() {
        println!("(nothing installed)");
    }
    for manifest in manifests {
        println!("{}  {} v{}", manifest.id, manifest.name, manifest.version);
    }
    Ok(())
}

fn collect_installed(root: &std::path::Path) -> Result<Vec<Manifest>, String> {
    let mut manifests = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let manifest_path = entry.path().join("current").join("manifest.json");
        let Ok(manifest_json) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<Manifest>(&manifest_json) else {
            continue;
        };
        manifests.push(manifest);
    }
    Ok(manifests)
}

// --- helpers ---

fn d_tag(event: &Event) -> Option<String> {
    tag_value(event, "d")
}

fn tag_value(event: &Event, name: &str) -> Option<String> {
    event
        .tags
        .iter()
        .find(|t| t.as_slice().first().map(String::as_str) == Some(name))
        .and_then(|t| t.as_slice().get(1).cloned())
}

fn short(pubkey_hex: &str) -> String {
    if pubkey_hex.len() > 12 {
        format!(
            "{}…{}",
            &pubkey_hex[..6],
            &pubkey_hex[pubkey_hex.len() - 6..]
        )
    } else {
        pubkey_hex.to_string()
    }
}

fn arg_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// `--json` switches the read commands (`trusted`/`available`/`installed`)
/// to machine-readable output, for the panel to shell out to rather than
/// parsing the human-readable text.
fn json_flag() -> bool {
    std::env::args().any(|a| a == "--json")
}
