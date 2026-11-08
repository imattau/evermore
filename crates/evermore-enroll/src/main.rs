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
use zbus::blocking::Connection;

const DESTINATION: &str = "org.freedesktop.home1";
const PATH: &str = "/org/freedesktop/home1";
const INTERFACE: &str = "org.freedesktop.home1.Manager";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("create") => create(&args[2..]),
        Some("activate") => activate(&args[2..]),
        Some("deactivate") => deactivate(&args[2..]),
        Some("remove") => remove(&args[2..]),
        _ => {
            eprintln!(
                "usage: evermore-enroll <create|activate|deactivate|remove>\n\n\
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
