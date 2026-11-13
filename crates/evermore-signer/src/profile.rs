//! Local, offline-first profile metadata (display name + avatar image).
//!
//! Two outputs from the same call, for two different readers:
//!
//! - A plain-file cache under `/var/lib/evermore-profiles/<pubkey-hex>/`
//!   that `evermore-greeter` (running as the dedicated `greeter` system
//!   user, before any human has logged in) can read directly. This is
//!   the one that actually matters for the greeter: the per-user local
//!   control-plane relay is a systemd *user* service that only starts
//!   after a login session exists (see `packaging/systemd/evermore-relay.service`),
//!   so there is no relay to query pre-login at all, for anyone.
//!
//!   Deliberately a *sibling* of `/var/lib/evermore`, not a child of
//!   it: that directory is already actively managed as a 0700,
//!   root-only secrets store by `evermore-pkcs11` and this crate's own
//!   `device_identity.rs`, both of which `set_permissions(dir, 0o700)`
//!   on it unconditionally on every bootstrap. Nesting a
//!   world-readable `profiles` subdirectory inside it would get
//!   silently re-locked the next time either of those ran.
//! - A real, signed `kind:0` event, best-effort published to that same
//!   local relay (see PROJECT_IDEA.md's Phase 5.5 -- "`kind:0` profile
//!   data is offline-first") for every other consumer (the panel, etc.)
//!   to read the normal Nostr-native way. Publish failures (relay not
//!   running yet, e.g. right after boot before the user unit is up) are
//!   logged, not fatal -- the file cache above is what's load-bearing.

use nostr::prelude::*;
use std::path::{Path, PathBuf};
use tungstenite::Message;

const PROFILES_DIR: &str = "/var/lib/evermore-profiles";
const DEFAULT_RELAY_URL: &str = "ws://127.0.0.1:4869";

#[derive(serde::Deserialize, serde::Serialize)]
struct ProfileCache {
    name: String,
    avatar: Option<String>,
}

/// Writes `display_name` (and, if given, a copy of the image at
/// `avatar_path`) to this identity's profile cache directory, then
/// best-effort publishes the equivalent `kind:0` event to the local
/// relay. `keys` is the identity's own keypair -- the cache directory
/// is named after its pubkey, exactly what `evermore-greeter` looks up
/// using the `evermore.pubkeyHex` field from the user's homed record.
pub fn set_profile(
    keys: &Keys,
    display_name: &str,
    avatar_path: Option<&Path>,
    picture_url: Option<&str>,
) -> Result<(), String> {
    let pubkey_hex = keys.public_key().to_hex();
    let dir = PathBuf::from(PROFILES_DIR).join(&pubkey_hex);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("creating {}: {e} -- has `evermore-profiles.conf` been applied with `systemd-tmpfiles --create`?", dir.display()))?;

    let avatar_filename = match avatar_path {
        Some(src) => Some(copy_avatar(src, &dir)?),
        None => cached_avatar(&dir),
    };

    write_cache_file(
        &dir,
        &ProfileCache {
            name: display_name.to_string(),
            avatar: avatar_filename.clone(),
        },
    )?;

    let mut metadata = serde_json::json!({ "name": display_name });
    if let Some(picture_url) = picture_url {
        metadata["picture"] = serde_json::json!(picture_url);
    } else if let Some(avatar_filename) = &avatar_filename {
        metadata["picture"] =
            serde_json::json!(format!("file://{}/{avatar_filename}", dir.display()));
    }
    let event = EventBuilder::new(Kind::Metadata, metadata.to_string())
        .finalize(keys)
        .map_err(|e| format!("signing kind:0 event: {e}"))?;
    publish_best_effort(&event);

    Ok(())
}

/// A name-only profile update should not silently remove an avatar that was
/// previously selected on this device.
fn cached_avatar(dir: &Path) -> Option<String> {
    let cache: ProfileCache =
        serde_json::from_str(&std::fs::read_to_string(dir.join("profile.json")).ok()?).ok()?;
    let filename = cache.avatar?;
    let path = Path::new(&filename);
    if path.file_name()?.to_str()? != filename.as_str() || !dir.join(&filename).is_file() {
        return None;
    }
    Some(filename)
}

fn copy_avatar(src: &Path, dir: &Path) -> Result<String, String> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| {
            e.eq_ignore_ascii_case("png")
                || e.eq_ignore_ascii_case("jpg")
                || e.eq_ignore_ascii_case("jpeg")
        })
        .ok_or_else(|| format!("avatar {} must be .png, .jpg, or .jpeg", src.display()))?;
    let filename = format!("avatar.{}", ext.to_ascii_lowercase());
    let dest = dir.join(&filename);
    let tmp = dest.with_extension("tmp");
    std::fs::copy(src, &tmp)
        .map_err(|e| format!("copying {} to {}: {e}", src.display(), tmp.display()))?;
    std::fs::rename(&tmp, &dest)
        .map_err(|e| format!("renaming {} into place: {e}", dest.display()))?;
    Ok(filename)
}

fn write_cache_file(dir: &Path, cache: &ProfileCache) -> Result<(), String> {
    let path = dir.join("profile.json");
    let tmp = dir.join("profile.json.tmp");
    let json = serde_json::to_string_pretty(cache).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("renaming {} into place: {e}", path.display()))
}

fn publish_best_effort(event: &Event) {
    match tungstenite::connect(DEFAULT_RELAY_URL) {
        Ok((mut ws, _)) => {
            let msg = serde_json::json!(["EVENT", event]).to_string();
            if let Err(e) = ws.send(Message::text(msg)) {
                eprintln!("evermore-signer: publishing kind:0 to local relay failed: {e}");
            }
        }
        Err(e) => {
            eprintln!(
                "evermore-signer: local relay not reachable ({e}) -- kind:0 not published, \
                 but the profile cache for the greeter was still written"
            );
        }
    }
}
