mod keyring_store;
mod server;

use nostr::prelude::*;
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("serve") => server::run().map_err(|e| e.to_string()),
        Some("keygen") => keygen(args.get(2)),
        Some("import") => import(args.get(2)),
        Some("list") => list(),
        _ => {
            eprintln!(
                "usage: evermore-signer <serve|keygen NAME|import NAME|list>\n\n\
                 serve        run the signing daemon (foreground)\n\
                 keygen NAME  generate a new identity, store it, print its npub\n\
                 import NAME  read an nsec (bech32 or hex) from stdin, store it\n\
                 list         list identities currently in the session keyring"
            );
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn require_name(name: Option<&String>) -> Result<&str, String> {
    name.map(String::as_str)
        .ok_or_else(|| "missing identity name".to_string())
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
