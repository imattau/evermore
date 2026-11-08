//! evermore-enroll: a thin CLI over `systemd-homed`'s real management API
//! (the system D-Bus `org.freedesktop.home1.Manager` interface -- verified
//! directly against a running VM; `homectl` itself talks to this same
//! interface, not varlink, which only exposes read-only user/group lookups
//! via `io.systemd.UserDatabase`).
//!
//! Exists because `homectl` drives secrets through systemd's interactive
//! `ask-password` agent even when a password hash is supplied in the
//! identity JSON -- it still needs the plaintext to derive the key that
//! encrypts the record's `secret` section. Calling `CreateHome`/
//! `ActivateHome` directly with the secret supplied inline in the request
//! sidesteps `ask-password` entirely, which matters for an automated
//! first-run enrollment flow with no human to answer a prompt.
//!
//! Deliberately stays a thin primitive: `create` takes an arbitrary
//! identity JSON file, the same shape `homectl -I` accepts (including
//! Evermore's own custom `"evermore": {...}` metadata block, verified to
//! round-trip through homed untouched). It doesn't hardcode password-based
//! auth as the only path -- a future FIDO2/PKCS#11-based identity record
//! goes through this same `create` command with a different JSON file, no
//! new code needed here.

use nostr::prelude::*;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use zbus::blocking::Connection;
use zeroize::{Zeroize, Zeroizing};

const DESTINATION: &str = "org.freedesktop.home1";
const PATH: &str = "/org/freedesktop/home1";
const INTERFACE: &str = "org.freedesktop.home1.Manager";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("create") => create(&args[2..]),
        Some("first-user") => first_user(),
        Some("activate") => activate(&args[2..]),
        Some("deactivate") => deactivate(&args[2..]),
        Some("remove") => remove(&args[2..]),
        _ => {
            eprintln!(
                "usage: evermore-enroll <first-user|create|activate|deactivate|remove>\n\n\
                 first-user  interactively create the first local identity and encrypted home\n\
                 create --identity PATH [--secret-password PASSWORD] [--identity-pubkey NPUB|HEX]\n\
                              create a home area from a JSON identity record\n\
                              (same shape as `homectl -I`). If\n\
                              --secret-password is given, merges it into the\n\
                              record's secret.password before sending, so\n\
                              homed never has to interactively ask for it.\n\
                              If --identity-pubkey is given, merges an\n\
                              \"evermore\":{{\"npub\":...,\"pubkeyHex\":...}}\n\
                              block into the record (see PROJECT_IDEA.md's\n\
                              Phase 5.5 -- \"the pubkey is the identity\"),\n\
                              overwriting any existing \"evermore\" block.\n\
                 activate --username NAME --password PASSWORD\n\
                              unlock and mount an inactive home area\n\
                 deactivate --username NAME\n\
                              lock and unmount an active home area\n\
                 remove --username NAME\n\
                              permanently remove a home area"
            );
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn first_user() -> Result<(), String> {
    if !nix_is_root() {
        return Err("first-user must run as root from the first-boot enrollment service".into());
    }
    if !list_homes()?.is_empty() {
        return Err("homed already contains a user; first-user is only for an empty system".into());
    }

    println!("Evermore first-user setup\n");
    let username = read_line("Account name (lowercase letters, numbers, _ or -): ")?;
    validate_username(&username)?;
    let display_name = read_line("Display name: ")?;
    if display_name.trim().is_empty() || display_name.contains(['\n', '\r', '\0']) {
        return Err("display name must not be empty or contain control characters".into());
    }

    let mode = read_line("Create a new identity or import an nsec? [new/import]: ")?;
    let keys = match mode.as_str() {
        "new" => Keys::generate(),
        "import" => {
            let secret = Zeroizing::new(ask_secret("Enter the existing nsec (input is hidden): ")?);
            let parsed = SecretKey::parse(secret.trim()).map_err(|e| format!("invalid nsec: {e}"));
            Keys::new(parsed?)
        }
        _ => return Err("choose 'new' or 'import'".into()),
    };
    let mut password = Zeroizing::new(ask_secret("Choose the encrypted home passphrase: ")?);
    let mut confirmation = Zeroizing::new(ask_secret("Repeat the home passphrase: ")?);
    if password.len() < 8 {
        password.zeroize();
        confirmation.zeroize();
        return Err("use a passphrase of at least 8 characters".into());
    }
    if password != confirmation {
        password.zeroize();
        confirmation.zeroize();
        return Err("passphrases do not match".into());
    }
    confirmation.zeroize();

    let pubkey = keys.public_key();
    let mut record = serde_json::json!({
        "userName": &username,
        "realName": &display_name,
        "homeDirectory": format!("/home/{username}"),
        "shell": "/bin/bash",
        "storage": "luks",
        "evermore": {
            "npub": pubkey.to_bech32().map_err(|e| e.to_string())?,
            "pubkeyHex": pubkey.to_hex(),
        },
        "secret": { "password": [password.as_str()] },
    });
    let mut record_json =
        Zeroizing::new(serde_json::to_string(&record).map_err(|e| e.to_string())?);
    let connection = system_connection()?;
    connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "CreateHome",
            &(record_json.as_str(),),
        )
        .map_err(|e| format!("CreateHome failed: {e}"))?;
    record_json.zeroize();
    if let Some(secret) = record.get_mut("secret")
        && let Some(passwords) = secret
            .get_mut("password")
            .and_then(serde_json::Value::as_array_mut)
    {
        passwords.clear();
    }
    activate_with_password(&username, &password)?;

    let (uid, gid) = list_homes()?
        .into_iter()
        .find(|home| home.0 == username)
        .map(|home| (home.1, home.3))
        .ok_or_else(|| "created home is missing from homed's user list".to_string())?;
    install_identity_secret(&username, uid, gid, &keys)?;
    write_greeter_profile(&pubkey.to_hex(), &display_name)?;

    println!("\nCreated the encrypted home for {display_name} ({username}).");
    println!(
        "Identity public key: {}",
        pubkey.to_bech32().unwrap_or_default()
    );
    if mode == "new" {
        println!("\nRecovery nsec (record this somewhere safe; it will not be shown again):");
        println!("{}", keys.secret_key().to_bech32().unwrap_or_default());
        let _ = read_line("Press Enter after you have recorded the recovery key: ");
    }
    write_first_user_marker()?;
    println!("First-user enrollment complete. Restart greetd to continue to login.");
    Ok(())
}

fn nix_is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .is_some_and(|output| output.status.success() && output.stdout.trim_ascii() == b"0")
}

fn validate_username(username: &str) -> Result<(), String> {
    let bytes = username.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 31
        || !(bytes[0].is_ascii_lowercase() || bytes[0] == b'_')
        || !bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
    {
        return Err("invalid account name; use 1–31 lowercase letters, digits, _ or -, starting with a letter or _".into());
    }
    Ok(())
}

fn read_line(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|e| e.to_string())?;
    Ok(value.trim().to_string())
}

fn ask_secret(prompt: &str) -> Result<String, String> {
    let output = std::process::Command::new("systemd-ask-password")
        .arg("--echo=no")
        .arg(prompt)
        .output()
        .map_err(|e| format!("starting systemd-ask-password: {e}"))?;
    if !output.status.success() {
        return Err("password prompt was cancelled".into());
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim_end_matches(['\r', '\n']).to_string())
        .map_err(|e| format!("password prompt returned invalid UTF-8: {e}"))
}

fn list_homes() -> Result<Vec<ListHomesEntry>, String> {
    let connection = system_connection()?;
    let reply = connection
        .call_method(Some(DESTINATION), PATH, Some(INTERFACE), "ListHomes", &())
        .map_err(|e| format!("ListHomes failed: {e}"))?;
    reply.body().deserialize().map_err(|e| e.to_string())
}

type ListHomesEntry = (
    String,
    u32,
    String,
    u32,
    String,
    String,
    String,
    zbus::zvariant::OwnedObjectPath,
);

fn activate_with_password(username: &str, password: &str) -> Result<(), String> {
    let secret_json = Zeroizing::new(
        serde_json::to_string(&serde_json::json!({ "password": [password] }))
            .map_err(|e| e.to_string())?,
    );
    let connection = system_connection()?;
    connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "ActivateHome",
            &(username, secret_json.as_str()),
        )
        .map_err(|e| format!("ActivateHome failed: {e}"))?;
    Ok(())
}

fn install_identity_secret(username: &str, uid: u32, gid: u32, keys: &Keys) -> Result<(), String> {
    let home = PathBuf::from(format!("/home/{username}"));
    let dir = home.join(".local/share/evermore");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    for path in [home.join(".local"), home.join(".local/share"), dir.clone()] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("setting permissions on {}: {e}", path.display()))?;
        chown(&path, uid, gid)?;
    }
    let path = dir.join("identity.nsec");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("creating {}: {e}", path.display()))?;
    let secret_hex = Zeroizing::new(keys.secret_key().to_secret_hex());
    file.write_all(secret_hex.as_bytes())
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    file.sync_all()
        .map_err(|e| format!("syncing {}: {e}", path.display()))?;
    drop(file);
    chown(&path, uid, gid)
}

fn chown(path: &std::path::Path, uid: u32, gid: u32) -> Result<(), String> {
    let status = std::process::Command::new("chown")
        .args([format!("{uid}:{gid}"), "--".into()])
        .arg(path)
        .status()
        .map_err(|e| format!("starting chown: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("chown failed for {}", path.display()))
    }
}

fn write_greeter_profile(pubkey_hex: &str, name: &str) -> Result<(), String> {
    let dir = PathBuf::from("/var/lib/evermore-profiles").join(pubkey_hex);
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let path = dir.join("profile.json");
    let data = serde_json::to_vec(&serde_json::json!({ "name": name, "avatar": null }))
        .map_err(|e| e.to_string())?;
    std::fs::write(path, data).map_err(|e| format!("writing greeter profile: {e}"))
}

fn write_first_user_marker() -> Result<(), String> {
    let dir = PathBuf::from("/var/lib/evermore");
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("setting permissions on {}: {e}", dir.display()))?;
    std::fs::write(dir.join("first-user-enrolled"), b"enrolled\n")
        .map_err(|e| format!("writing first-user marker: {e}"))
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn require_flag(args: &[String], flag: &str) -> Result<String, String> {
    flag_value(args, flag).ok_or_else(|| format!("missing {flag}"))
}

fn system_connection() -> Result<Connection, String> {
    Connection::system().map_err(|e| format!("connecting to the system bus: {e}"))
}

fn create(args: &[String]) -> Result<(), String> {
    let identity_path = require_flag(args, "--identity")?;
    let raw = std::fs::read_to_string(&identity_path)
        .map_err(|e| format!("reading {identity_path}: {e}"))?;
    let mut record: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parsing {identity_path}: {e}"))?;

    if let Some(password) = flag_value(args, "--secret-password") {
        let secret = record
            .as_object_mut()
            .ok_or("identity record must be a JSON object")?
            .entry("secret")
            .or_insert_with(|| serde_json::json!({}));
        secret
            .as_object_mut()
            .ok_or("'secret' field must be a JSON object")?
            .insert("password".to_string(), serde_json::json!([password]));
    }

    if let Some(pubkey_arg) = flag_value(args, "--identity-pubkey") {
        let pubkey = PublicKey::parse(&pubkey_arg)
            .map_err(|e| format!("invalid --identity-pubkey '{pubkey_arg}': {e}"))?;
        record
            .as_object_mut()
            .ok_or("identity record must be a JSON object")?
            .insert(
                "evermore".to_string(),
                serde_json::json!({
                    "npub": pubkey.to_bech32().map_err(|e| e.to_string())?,
                    "pubkeyHex": pubkey.to_hex(),
                }),
            );
    }

    let record_json = serde_json::to_string(&record).map_err(|e| e.to_string())?;
    let connection = system_connection()?;
    connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "CreateHome",
            &(record_json,),
        )
        .map_err(|e| format!("CreateHome failed: {e}"))?;
    println!("created home area from {identity_path}");
    Ok(())
}

fn activate(args: &[String]) -> Result<(), String> {
    let username = require_flag(args, "--username")?;
    let password = require_flag(args, "--password")?;
    let secret_json = serde_json::to_string(&serde_json::json!({ "password": [password] }))
        .map_err(|e| e.to_string())?;

    let connection = system_connection()?;
    connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "ActivateHome",
            &(username.as_str(), secret_json.as_str()),
        )
        .map_err(|e| format!("ActivateHome failed: {e}"))?;
    println!("activated {username}");
    Ok(())
}

fn deactivate(args: &[String]) -> Result<(), String> {
    let username = require_flag(args, "--username")?;
    let connection = system_connection()?;
    connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "DeactivateHome",
            &(username.as_str(),),
        )
        .map_err(|e| format!("DeactivateHome failed: {e}"))?;
    println!("deactivated {username}");
    Ok(())
}

fn remove(args: &[String]) -> Result<(), String> {
    let username = require_flag(args, "--username")?;
    let connection = system_connection()?;
    connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "RemoveHome",
            &(username.as_str(),),
        )
        .map_err(|e| format!("RemoveHome failed: {e}"))?;
    println!("removed {username}");
    Ok(())
}
