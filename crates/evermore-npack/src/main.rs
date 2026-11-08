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
use std::path::{Path, PathBuf};

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
        Some("update") => update_cmd(args.next()),
        Some("update-all") => update_all_cmd(),
        Some("remove") => remove_cmd(args.next()),
        Some("installed") => installed_cmd(),
        _ => {
            eprintln!(
                "usage: evermore-npack <trust|untrust|trusted|publish|available|install|update|update-all|remove|installed>"
            );
            eprintln!("  trust PUBKEY_HEX --identity NAME [--relay URL]");
            eprintln!("  untrust PUBKEY_HEX --identity NAME [--relay URL]");
            eprintln!("  trusted --identity NAME [--relay URL]");
            eprintln!(
                "  publish MANIFEST.json PAYLOAD_DIR --identity NAME --blossom-url URL [--relay URL]"
            );
            eprintln!("  available --identity NAME [--relay URL]");
            eprintln!("  install PACKAGE_ID --identity NAME --blossom-url URL [--relay URL]");
            eprintln!("  update PACKAGE_ID --identity NAME --blossom-url URL [--relay URL]");
            eprintln!("  update-all --identity NAME --blossom-url URL [--relay URL]");
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

// --- install / update / remove / installed ---

/// Resolves which release to fetch: a pinned `(version, sha256)` pair
/// checked against a trusted signed event, or (when neither is given)
/// that publisher set's latest. Shared by `install` (where pinning makes
/// sense -- installing nothing yet, you may want a specific known-good
/// build) and `update` (which always moves to latest).
fn resolve_release(
    relay: &str,
    id: &str,
    trust_list: &[String],
    pinned_version: Option<&str>,
    pinned_sha256: Option<&str>,
) -> Result<(ReleaseListing, serde_json::Value), String> {
    let resolved = if let (Some(version), Some(sha256)) = (pinned_version, pinned_sha256) {
        if !safe_package_component(version)
            || sha256.len() != 64
            || !sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("pinned version or SHA-256 is malformed".into());
        }
        let events = relay_client::query(
            relay,
            serde_json::json!({
                "kinds": [evermore_npack::KIND_RELEASE_ARTIFACT_SET], "authors": trust_list, "#d": [id]
            }),
        )?;
        events
            .into_iter()
            .find_map(|event| {
                let candidate = ReleaseListing {
                    id: id.to_string(),
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
        fetch_latest_release_event(relay, id, trust_list)?
            .ok_or_else(|| format!("no release of '{id}' found from a trusted publisher"))?
    };

    let (release, event) = resolved;
    if !safe_package_component(&release.version)
        || release.sha256.len() != 64
        || !release.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("trusted release contains an unsafe version or malformed SHA-256".into());
    }
    Ok((release, event))
}

/// Downloads a resolved release's archive and checks it against the
/// signed SHA-256 -- the actual trust boundary, independent of whichever
/// Blossom URL answered.
fn download_verified(blossom_url: &str, release: &ReleaseListing) -> Result<Vec<u8>, String> {
    let archive_bytes = blossom_client::download(blossom_url, &release.sha256)
        .or_else(|_| blossom_client::download(&release.blossom_url, &release.sha256))?;
    let actual_sha256 = blossom_client::hex_sha256(&archive_bytes);
    if !actual_sha256.eq_ignore_ascii_case(&release.sha256) {
        return Err(format!(
            "downloaded content doesn't match the signed release (expected {}, got {actual_sha256}) -- refusing to install",
            release.sha256
        ));
    }
    Ok(archive_bytes)
}

/// Unpacks a verified archive into its own version directory and checks
/// the manifest matches what was requested. Does not touch `current` or
/// any symlinks -- callers decide when to switch over.
fn unpack_release(
    id: &str,
    release: &ReleaseListing,
    archive_bytes: &[u8],
    signed_release_event: &serde_json::Value,
) -> Result<(PathBuf, Manifest), String> {
    let version_dir = install_root().join(id).join(&release.version);
    let manifest = archive::extract(archive_bytes, &version_dir)?;
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
        serde_json::to_vec(signed_release_event).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("writing signed release evidence: {e}"))?;
    std::fs::write(version_dir.join("package.npk"), archive_bytes)
        .map_err(|e| format!("retaining verified package archive: {e}"))?;

    Ok((version_dir, manifest))
}

/// Refuses to activate a package whose declared `dependencies` aren't
/// already installed -- the manifest field existed from the start but
/// nothing enforced it, so a package naming a dependency that isn't
/// present would previously install anyway and fail at runtime instead.
fn check_dependencies(manifest: &Manifest) -> Result<(), String> {
    if manifest.dependencies.is_empty() {
        return Ok(());
    }
    let installed: std::collections::HashSet<String> = collect_installed(&install_root())
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.id)
        .collect();
    let missing: Vec<&String> = manifest
        .dependencies
        .iter()
        .filter(|dep| !installed.contains(*dep))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} depends on {} which {} not installed -- install {} first",
            manifest.id,
            missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            if missing.len() == 1 { "is" } else { "are" },
            if missing.len() == 1 { "it" } else { "them" },
        ))
    }
}

/// Switches `current` to `version_dir`, re-links `bin`, and
/// enables/restarts the `systemd_unit` if the manifest declares one.
/// `is_update` only changes whether the unit is started fresh
/// (`enable --now`) or bounced (`restart`) -- everything else is
/// identical between first install and a later update, which is the
/// point: a baked-in package updates through the exact same path as one
/// `npack install`ed from cold.
fn activate(
    id: &str,
    version_dir: &Path,
    manifest: &Manifest,
    is_update: bool,
) -> Result<(), String> {
    let current_link = install_root().join(id).join("current");
    let _ = std::fs::remove_file(&current_link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(version_dir, &current_link)
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

    if let Some(unit) = &manifest.systemd_unit {
        activate_systemd_unit(version_dir, unit, is_update);
    }

    Ok(())
}

/// Best-effort: a failure here (no systemd user session, e.g. this CLI
/// running inside a container or a build step) must not undo the
/// already-verified install/update -- the files are correctly in place
/// either way, so this only warns rather than returning an error.
fn activate_systemd_unit(version_dir: &Path, unit: &str, is_update: bool) {
    let unit_name = match unit.rsplit('/').next() {
        Some(name) if !name.is_empty() => name,
        _ => {
            eprintln!("evermore-npack: warning: malformed systemd_unit '{unit}'");
            return;
        }
    };
    let src = version_dir.join(unit);
    let unit_dir = evermore_npack::systemd_user_unit_dir();
    let dest = unit_dir.join(unit_name);

    let install_result =
        std::fs::create_dir_all(&unit_dir).and_then(|()| std::fs::copy(&src, &dest).map(|_| ()));
    if let Err(e) = install_result {
        eprintln!("evermore-npack: warning: couldn't install {unit_name}: {e}");
        return;
    }

    let reload = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    if let Err(e) = reload {
        eprintln!("evermore-npack: warning: `systemctl --user daemon-reload` failed: {e}");
        return;
    }

    let verb = if is_update {
        vec!["--user", "restart", unit_name]
    } else {
        vec!["--user", "enable", "--now", unit_name]
    };
    match std::process::Command::new("systemctl").args(&verb).status() {
        Ok(status) if status.success() => {
            println!(
                "{} {unit_name}",
                if is_update { "restarted" } else { "enabled" }
            );
        }
        Ok(status) => eprintln!(
            "evermore-npack: warning: `systemctl {}` exited with {status}",
            verb.join(" ")
        ),
        Err(e) => eprintln!("evermore-npack: warning: couldn't run systemctl: {e}"),
    }
}

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
    let (release, signed_release_event) = resolve_release(
        &relay,
        &id,
        &trust_list,
        requested_version.as_deref(),
        requested_sha256.as_deref(),
    )?;

    let archive_bytes = download_verified(&blossom_url, &release)?;
    let (version_dir, manifest) =
        unpack_release(&id, &release, &archive_bytes, &signed_release_event)?;
    check_dependencies(&manifest)?;
    activate(&id, &version_dir, &manifest, false)?;

    println!(
        "installed {} {} from {}",
        manifest.id,
        manifest.version,
        short(&release.publisher_pubkey)
    );
    Ok(())
}

/// Reads the manifest a package is currently running, if any -- the
/// source of truth is the filesystem (`current/manifest.json`), same as
/// `installed_cmd`, not a separate state file that could drift from it.
fn current_manifest(id: &str) -> Option<Manifest> {
    let manifest_json = std::fs::read_to_string(
        install_root()
            .join(id)
            .join("current")
            .join("manifest.json"),
    )
    .ok()?;
    serde_json::from_str(&manifest_json).ok()
}

fn update_cmd(id: Option<String>) -> Result<(), String> {
    let id = require(id, "PACKAGE_ID")?;
    if !safe_package_component(&id) {
        return Err("PACKAGE_ID contains characters unsafe for an install path".into());
    }
    let blossom_url = arg_value("--blossom-url").ok_or("missing --blossom-url URL")?;
    let signer = identity_arg()?;
    let relay = relay_arg();
    let own_pubkey = signer.get_public_key()?;

    let installed = current_manifest(&id)
        .ok_or_else(|| format!("'{id}' isn't installed -- use `install` first"))?;

    let trust_list = fetch_trust_list(&relay, &own_pubkey)?;
    if trust_list.is_empty() {
        return Err("no trusted publishers -- `evermore-npack trust PUBKEY_HEX` first".into());
    }
    let (release, signed_release_event) = resolve_release(&relay, &id, &trust_list, None, None)?;

    if release.version == installed.version {
        println!("{id} already at v{}", installed.version);
        return Ok(());
    }

    let archive_bytes = download_verified(&blossom_url, &release)?;
    let (version_dir, manifest) =
        unpack_release(&id, &release, &archive_bytes, &signed_release_event)?;
    check_dependencies(&manifest)?;
    activate(&id, &version_dir, &manifest, true)?;

    println!(
        "updated {} {} -> {} from {}",
        id,
        installed.version,
        manifest.version,
        short(&release.publisher_pubkey)
    );
    Ok(())
}

/// Walks every currently-installed package and updates each in turn.
/// One package's failure (no newer trusted release, a dependency not
/// satisfied, a download hiccup) is reported and skipped rather than
/// aborting the rest -- these are independent packages precisely so a
/// problem with one doesn't block updating the others.
fn update_all_cmd() -> Result<(), String> {
    let root = install_root();
    let manifests = if root.is_dir() {
        collect_installed(&root)?
    } else {
        Vec::new()
    };
    if manifests.is_empty() {
        println!("(nothing installed)");
        return Ok(());
    }
    for manifest in manifests {
        match update_cmd(Some(manifest.id.clone())) {
            Ok(()) => {}
            Err(e) => eprintln!("evermore-npack: {} not updated: {e}", manifest.id),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `install_root()` reads `XDG_DATA_HOME`, a process-global env var, so
    // tests that need to control it run one at a time under this lock
    // rather than racing each other's directories.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn bare_manifest(id: &str, dependencies: Vec<String>) -> Manifest {
        Manifest {
            id: id.to_string(),
            name: id.to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            bin: None,
            systemd_unit: None,
            ai_model: None,
            dependencies,
        }
    }

    fn with_data_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "evermore-npack-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var("XDG_DATA_HOME").ok();
        // SAFETY: serialized by ENV_LOCK -- no other thread reads or
        // writes process env vars while this guard is held.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }
        let result = f();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn dependency_check_passes_once_dependency_is_installed() {
        with_data_home(|| {
            let needs_dep =
                bare_manifest("sh.evermore.mcp", vec!["sh.evermore.operationsd".into()]);

            let err = check_dependencies(&needs_dep).unwrap_err();
            assert!(err.contains("sh.evermore.operationsd"));

            let dep = bare_manifest("sh.evermore.operationsd", vec![]);
            let dep_dir = install_root().join(&dep.id).join(&dep.version);
            std::fs::create_dir_all(&dep_dir).unwrap();
            std::fs::write(
                dep_dir.join("manifest.json"),
                serde_json::to_string(&dep).unwrap(),
            )
            .unwrap();
            let current = install_root().join(&dep.id).join("current");
            std::os::unix::fs::symlink(&dep_dir, &current).unwrap();

            assert!(check_dependencies(&needs_dep).is_ok());
        });
    }

    #[test]
    fn dependency_check_is_a_no_op_without_declared_dependencies() {
        with_data_home(|| {
            assert!(check_dependencies(&bare_manifest("sh.evermore.panel", vec![])).is_ok());
        });
    }

    #[test]
    fn current_manifest_reads_through_the_symlink() {
        with_data_home(|| {
            assert!(current_manifest("sh.evermore.panel").is_none());

            let manifest = bare_manifest("sh.evermore.panel", vec![]);
            let version_dir = install_root().join(&manifest.id).join(&manifest.version);
            std::fs::create_dir_all(&version_dir).unwrap();
            std::fs::write(
                version_dir.join("manifest.json"),
                serde_json::to_string(&manifest).unwrap(),
            )
            .unwrap();
            let current = install_root().join(&manifest.id).join("current");
            std::os::unix::fs::symlink(&version_dir, &current).unwrap();

            let read_back = current_manifest(&manifest.id).unwrap();
            assert_eq!(read_back.version, "1.0.0");
        });
    }
}

#[cfg(test)]
mod manifest_fixture_tests {
    use super::Manifest;

    #[test]
    fn packaged_manifests_parse_and_reference_real_units() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/npack");
        let mut checked = 0;
        for entry in std::fs::read_dir(&root).unwrap() {
            let entry = entry.unwrap();
            if !entry.path().is_dir() {
                continue;
            }
            let manifest_path = entry.path().join("manifest.json");
            let json = std::fs::read_to_string(&manifest_path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", manifest_path.display()));
            let manifest: Manifest = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("parsing {}: {e}", manifest_path.display()));
            assert_eq!(
                manifest.id,
                format!(
                    "sh.evermore.{}",
                    entry
                        .file_name()
                        .to_string_lossy()
                        .trim_start_matches("sh.evermore.")
                )
            );
            if let Some(unit) = &manifest.systemd_unit {
                let unit_path = root.join("../systemd").join(unit);
                assert!(
                    unit_path.is_file(),
                    "{} declares systemd_unit {unit} but {} doesn't exist",
                    manifest_path.display(),
                    unit_path.display()
                );
            }
            checked += 1;
        }
        assert_eq!(
            checked, 11,
            "expected 11 packaged manifests under packaging/npack"
        );
    }
}
