mod device_identity;
mod keyring_store;
mod server;

use nostr::prelude::*;
use std::io::Read;
use std::path::PathBuf;

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
        Some("list") => list(),
        Some("bootstrap-device") => bootstrap_device(&args[2..]),
        _ => {
            eprintln!(
                "usage: evermore-signer <serve|keygen NAME|import NAME|list|bootstrap-device>\n\n\
                 serve [--device] [--state-dir PATH] [--socket PATH]\n\
                              run the signing daemon (foreground). --device\n\
                              loads the device-root identity instead of serving\n\
                              human identities, on its own socket.\n\
                 keygen NAME  generate a new identity, store it, print its npub\n\
                 import NAME  read an nsec (bech32 or hex) from stdin, store it\n\
                 list         list identities currently in the session keyring\n\
                 bootstrap-device [--state-dir PATH]\n\
                              generate the device-root identity if one doesn't\n\
                              already exist; idempotent, prints its pubkey"
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
    Ok(name)
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
