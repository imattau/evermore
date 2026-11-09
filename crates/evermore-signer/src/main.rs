mod backing;
mod device_identity;
mod keyring_store;
mod profile;
mod server;

use nostr::prelude::*;
use std::io::Read;
use std::path::PathBuf;
use zeroize::Zeroizing;

/// Default socket path for `serve --device` -- the device signer is a
/// separate, system-scoped process from the per-user daemon (see
/// `device_identity.rs`), so it gets its own runtime directory under
/// `/run` rather than a user's `$XDG_RUNTIME_DIR`.
const DEFAULT_DEVICE_SOCKET_PATH: &str = "/run/evermore/device-signer.sock";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("serve") => serve(&args[2..]),
        Some("keygen") => keygen(args.get(2)),
        Some("import") => import(args.get(2)),
        Some("load-home-identity") => load_home_identity(args.get(2), &args[3..]),
        Some("unload-home-identity") => unload_home_identity(args.get(2)),
        Some("list") => list(),
        Some("bootstrap-device") => bootstrap_device(&args[2..]),
        Some("pair") => pair(args.get(2), &args[3..]),
        Some("list-remote") => list_remote(),
        Some("set-profile") => set_profile(args.get(2), &args[3..]),
        _ => {
            eprintln!(
                "usage: evermore-signer <serve|keygen NAME|import NAME|load-home-identity NAME|unload-home-identity NAME|list|bootstrap-device|pair|list-remote|set-profile>\n\n\
                 serve [--device] [--state-dir PATH] [--socket PATH]\n\
                              run the signing daemon (foreground). --device\n\
                              loads the device-root identity instead of serving\n\
                              human identities, on its own socket.\n\
                 keygen NAME  generate a new identity, store it, print its npub\n\
                 import NAME  read an nsec (bech32 or hex) from stdin, store it\n\
                 load-home-identity NAME [--path PATH] [--if-present]\n\
                              read the mode-0600 encrypted-home key file into\n\
                              this session's keyring before starting the daemon\n\
                 unload-home-identity NAME\n\
                              unlink and revoke the identity key at service stop\n\
                 list         list local identities currently in the session keyring\n\
                 bootstrap-device [--state-dir PATH]\n\
                              generate the device-root identity if one doesn't\n\
                              already exist; idempotent, prints its pubkey\n\
                 pair NAME --relay URL --remote-pubkey HEX\n\
                              register NAME as backed by a remote NIP-46\n\
                              signer -- no secret for it is ever stored here.\n\
                              Remote signing itself is not implemented yet;\n\
                              signing requests for a paired identity fail with\n\
                              a clear error until it is.\n\
                 list-remote  list identities paired to a remote signer\n\
                 set-profile NAME --display-name NAME [--avatar PATH]\n\
                              set this identity's display name/avatar: writes\n\
                              the pre-login cache evermore-greeter reads\n\
                              (/var/lib/evermore-profiles/<pubkey-hex>/) and\n\
                              best-effort publishes a kind:0 event to the\n\
                              local relay. --avatar must be .png/.jpg/.jpeg."
            );
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn require_flag(args: &[String], flag: &str) -> Result<String, String> {
    flag_value(args, flag).ok_or_else(|| format!("missing {flag}"))
}

fn state_dir_arg(args: &[String]) -> PathBuf {
    flag_value(args, "--state-dir")
        .map(PathBuf::from)
        .unwrap_or_else(device_identity::default_state_dir)
}

fn serve(args: &[String]) -> Result<(), String> {
    if has_flag(args, "--device") {
        let state_dir = state_dir_arg(args);
        device_identity::load_into_keyring(&state_dir)?;
        let socket_path = flag_value(args, "--socket")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DEVICE_SOCKET_PATH));
        server::run(socket_path).map_err(|e| e.to_string())
    } else {
        server::run(evermore_common::runtime_socket_path()).map_err(|e| e.to_string())
    }
}

fn bootstrap_device(args: &[String]) -> Result<(), String> {
    let state_dir = state_dir_arg(args);
    let pubkey = device_identity::bootstrap(&state_dir)?;
    println!("device identity state dir: {}", state_dir.display());
    println!("pubkey:    {}", pubkey.to_hex());
    println!("npub:      {}", pubkey.to_bech32().unwrap_or_default());
    Ok(())
}

fn require_name(name: Option<&String>) -> Result<&str, String> {
    let name = name
        .map(String::as_str)
        .ok_or_else(|| "missing identity name".to_string())?;
    if name == device_identity::RESERVED_IDENTITY_NAME {
        return Err(format!(
            "'{name}' is reserved for the device-root identity -- use `bootstrap-device` instead"
        ));
    }
    if matches!(backing::backing_for(name), backing::Backing::Remote { .. }) {
        return Err(format!(
            "'{name}' is paired to a remote NIP-46 signer -- a local secret must never be \
             generated or imported for it (that's the entire point of pairing it)"
        ));
    }
    Ok(name)
}

fn pair(name: Option<&String>, args: &[String]) -> Result<(), String> {
    let name = name
        .map(String::as_str)
        .ok_or_else(|| "missing identity name".to_string())?;
    if name == device_identity::RESERVED_IDENTITY_NAME {
        return Err(format!(
            "'{name}' is reserved for the device-root identity -- it can never be remote-backed"
        ));
    }
    let relay = flag_value(args, "--relay").ok_or("missing --relay URL")?;
    let remote_pubkey = flag_value(args, "--remote-pubkey").ok_or("missing --remote-pubkey HEX")?;
    if keyring_store::exists(name).map_err(|e| e.to_string())? {
        return Err(format!(
            "'{name}' already has a local secret in this session's keyring -- remove it first \
             (there is no `remove` yet; restart the session) before pairing it to a remote signer"
        ));
    }
    backing::pair_remote(name, &relay, &remote_pubkey)?;
    println!("'{name}' is now paired to a remote NIP-46 signer");
    println!("relay:         {relay}");
    println!("remote pubkey: {remote_pubkey}");
    Ok(())
}

fn list_remote() -> Result<(), String> {
    let registered = backing::list_registered();
    let mut names: Vec<&String> = registered.keys().collect();
    names.sort();
    if names.is_empty() {
        println!("(no identities paired to a remote signer)");
    }
    for name in names {
        if let backing::Backing::Remote {
            relay,
            remote_signer_pubkey,
        } = &registered[name]
        {
            println!("{name}  relay={relay}  remote_pubkey={remote_signer_pubkey}");
        }
    }
    Ok(())
}

fn set_profile(name: Option<&String>, args: &[String]) -> Result<(), String> {
    let name = name
        .map(String::as_str)
        .ok_or_else(|| "missing identity name".to_string())?;
    let display_name = require_flag(args, "--display-name")?;
    let avatar = flag_value(args, "--avatar").map(PathBuf::from);

    let bytes = keyring_store::load(name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!("unknown identity '{name}' (no secret in this session's keyring)")
        })?;
    let secret_key = SecretKey::from_slice(&bytes).map_err(|e| e.to_string())?;
    let keys = Keys::new(secret_key);

    profile::set_profile(&keys, &display_name, avatar.as_deref())?;
    println!("profile set for '{name}' ({})", keys.public_key().to_hex());
    Ok(())
}

fn keygen(name: Option<&String>) -> Result<(), String> {
    let name = require_name(name)?;
    if keyring_store::exists(name).map_err(|e| e.to_string())? {
        return Err(format!(
            "identity '{name}' already exists (use `import` to overwrite, or pick a different name)"
        ));
    }
    let keys = Keys::generate();
    keyring_store::store(name, keys.secret_key().as_secret_bytes()).map_err(|e| e.to_string())?;
    println!("identity:  {name}");
    println!("pubkey:    {}", keys.public_key().to_hex());
    println!(
        "npub:      {}",
        keys.public_key().to_bech32().unwrap_or_default()
    );
    Ok(())
}

fn import(name: Option<&String>) -> Result<(), String> {
    let name = require_name(name)?;
    eprintln!("paste an nsec (bech32 or hex), then press Enter:");
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    let secret_key = SecretKey::parse(input.trim()).map_err(|e| e.to_string())?;
    let keys = Keys::new(secret_key);
    keyring_store::store(name, keys.secret_key().as_secret_bytes()).map_err(|e| e.to_string())?;
    println!("identity:  {name}");
    println!("pubkey:    {}", keys.public_key().to_hex());
    println!(
        "npub:      {}",
        keys.public_key().to_bech32().unwrap_or_default()
    );
    Ok(())
}

fn load_home_identity(name: Option<&String>, args: &[String]) -> Result<(), String> {
    let name = require_name(name)?;
    let path = flag_value(args, "--path")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".local/share/evermore/identity.nsec")
        });
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && has_flag(args, "--if-present") =>
        {
            return Ok(());
        }
        Err(error) => return Err(format!("reading {}: {error}", path.display())),
    };
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|e| format!("inspecting {}: {e}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be a regular file with no group or other permissions",
            path.display()
        ));
    }
    let secret = std::str::from_utf8(&bytes)
        .map_err(|e| format!("{} is not UTF-8: {e}", path.display()))?
        .trim();
    let keys =
        Keys::new(SecretKey::parse(secret).map_err(|e| format!("invalid identity key: {e}"))?);
    keyring_store::store(name, keys.secret_key().as_secret_bytes()).map_err(|e| e.to_string())?;
    eprintln!(
        "loaded home identity '{}' ({})",
        name,
        keys.public_key().to_hex()
    );
    Ok(())
}

fn unload_home_identity(name: Option<&String>) -> Result<(), String> {
    let name = require_name(name)?;
    keyring_store::remove(name).map_err(|e| e.to_string())
}

fn list() -> Result<(), String> {
    let names = keyring_store::list().map_err(|e| e.to_string())?;
    if names.is_empty() {
        println!("(no identities loaded)");
    }
    for name in names {
        println!("{name}");
    }
    Ok(())
}
