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
use std::io::{self, Read, Write};
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
        Some("first-user-create") => first_user_create(),
        Some("first-user-pair") => first_user_pair(),
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

/// Create the first account from a GUI request sent on stdin. The command
/// is invoked through a narrowly-scoped sudoers rule; secrets never appear
/// in argv or a temporary file. The greeter receives only the public key.
fn first_user_create() -> Result<(), String> {
    if !nix_is_root() {
        return Err("first-user-create must run as root".into());
    }
    if std::env::var("SUDO_USER").as_deref() != Ok("greeter") {
        return Err("first-user-create may only be called by the greeter".into());
    }

    let mut raw = Zeroizing::new(String::new());
    io::stdin()
        .read_to_string(&mut raw)
        .map_err(|e| format!("reading first-user request: {e}"))?;
    let mut request: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("invalid first-user request: {e}"))?;
    raw.zeroize();

    let username = request
        .get("username")
        .and_then(serde_json::Value::as_str)
        .ok_or("first-user request is missing username")?
        .to_owned();
    let display_name = request
        .get("display_name")
        .and_then(serde_json::Value::as_str)
        .ok_or("first-user request is missing display_name")?
        .to_owned();
    let secret_hex = take_secret(&mut request, "identity_secret")?;
    let password = take_secret(&mut request, "passphrase")?;
    let pairing_enabled = request
        .get("pairing_enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let lock_path = PathBuf::from("/run/evermore-first-user.lock");
    let _lock = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|e| format!("first-user setup is already running or cannot create lock: {e}"))?;
    let result = create_first_user_account(
        &username,
        &display_name,
        &secret_hex,
        &password,
        pairing_enabled,
    );
    let _ = std::fs::remove_file(lock_path);
    drop(request);
    result?;
    println!("{{\"ok\":true}}");
    Ok(())
}

/// Begin the NIP-46 connection as the persistent device client identity.
/// The first JSON line contains the QR payload; the second reports a
/// validated connect + get_public_key exchange.
fn first_user_pair() -> Result<(), String> {
    if !nix_is_root() || std::env::var("SUDO_USER").as_deref() != Ok("greeter") {
        return Err("first-user-pair may only be called by the greeter through sudo".into());
    }
    if !list_homes()?.is_empty() || PathBuf::from("/var/lib/evermore/first-user-enrolled").exists()
    {
        return Err("first-user pairing is only available during first-user setup".into());
    }
    ensure_pkcs11_module()?;
    let mut relay = String::new();
    io::stdin()
        .read_to_string(&mut relay)
        .map_err(|e| e.to_string())?;
    let relay = relay.trim();

    let bootstrap = std::process::Command::new("/usr/bin/evermore-signer")
        .arg("bootstrap-device")
        .output()
        .map_err(|e| format!("starting device identity bootstrap: {e}"))?;
    if !bootstrap.status.success() {
        return Err(format!(
            "device identity bootstrap failed: {}",
            String::from_utf8_lossy(&bootstrap.stderr).trim()
        ));
    }
    let state_dir = PathBuf::from("/var/lib/evermore");
    let key_text = Zeroizing::new(
        std::fs::read_to_string(state_dir.join("device-identity"))
            .map_err(|e| format!("reading device identity: {e}"))?,
    );
    let keys = Keys::new(SecretKey::parse(key_text.trim()).map_err(|e| e.to_string())?);
    let (remote, user_pubkey) = pair_over_relay(relay, &keys, |uri| {
        #[derive(serde::Serialize)]
        struct Waiting<'a> {
            state: &'a str,
            uri: &'a str,
        }
        let message = Zeroizing::new(
            serde_json::to_string(&Waiting {
                state: "waiting",
                uri,
            })
            .map_err(|e| e.to_string())?,
        );
        println!("{}", message.as_str());
        io::stdout().flush().map_err(|e| e.to_string())
    })?;
    write_pairing(&state_dir, relay, &remote.to_hex())?;
    println!(
        "{}",
        serde_json::json!({"state":"paired", "remote_signer_pubkey":remote.to_hex(), "user_pubkey":user_pubkey})
    );
    Ok(())
}

fn pair_over_relay(
    relay: &str,
    keys: &Keys,
    on_uri: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(PublicKey, String), String> {
    let relay_url = RelayUrl::parse(relay).map_err(|e| format!("invalid relay URL: {e}"))?;
    let uri = NostrConnectUri::client(keys.public_key(), [relay_url], "Evermore");
    let uri_text = Zeroizing::new(format!("{}&perms=get_public_key%2Csign_event%3A31337", uri));
    let secret = match uri {
        NostrConnectUri::Client { secret, .. } => Zeroizing::new(secret),
        _ => return Err("failed to construct NIP-46 client URI".into()),
    };

    let (mut ws, _) = tungstenite::connect(relay)
        .map_err(|e| format!("could not connect to relay {relay}: {e}"))?;
    match ws.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(tcp) => {
            let _ = tcp.set_read_timeout(Some(std::time::Duration::from_millis(250)));
        }
        tungstenite::stream::MaybeTlsStream::Rustls(tls) => {
            let _ = tls
                .sock
                .set_read_timeout(Some(std::time::Duration::from_millis(250)));
        }
        _ => {}
    }
    let filter = Filter::new()
        .kind(Kind::NostrConnect)
        .pubkey(keys.public_key())
        .since(Timestamp::now() - 10);
    ws.send(tungstenite::Message::text(
        serde_json::json!(["REQ", "evermore-first-user-pair", filter]).to_string(),
    ))
    .map_err(|e| format!("subscribing to relay: {e}"))?;
    on_uri(uri_text.as_str())?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let remote = loop {
        if std::time::Instant::now() >= deadline {
            return Err("pairing timed out after five minutes".into());
        }
        let text = match ws.read() {
            Ok(tungstenite::Message::Text(text)) => text,
            Ok(tungstenite::Message::Close(_)) => return Err("relay closed the connection".into()),
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(e) => return Err(format!("relay connection failed: {e}")),
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if value.get(0).and_then(|v| v.as_str()) != Some("EVENT") {
            continue;
        }
        let Some(raw_event) = value.get(2) else {
            continue;
        };
        let Ok(event) = serde_json::from_value::<Event>(raw_event.clone()) else {
            continue;
        };
        if event.kind != Kind::NostrConnect
            || !event.tags.iter().any(|t| {
                t.kind() == "p" && t.content() == Some(keys.public_key().to_hex().as_str())
            })
        {
            continue;
        }
        if event.verify().is_err() {
            continue;
        }
        let Ok(plaintext) = keys.nip44_decrypt(&event.pubkey, &event.content) else {
            continue;
        };
        let plaintext = Zeroizing::new(plaintext);
        let Ok(message) = NostrConnectMessage::from_json(&plaintext) else {
            continue;
        };
        match message {
            NostrConnectMessage::Response {
                mut result,
                error: None,
                ..
            } if result.as_deref() == Some(secret.as_str()) => {
                if let Some(result) = &mut result {
                    result.zeroize();
                }
                break event.pubkey;
            }
            NostrConnectMessage::Response {
                error: Some(err), ..
            } => return Err(format!("phone rejected pairing: {err}")),
            _ => continue,
        }
    };

    let response = pairing_request(&mut ws, keys, remote, NostrConnectRequest::GetPublicKey)?;
    if let Some(error) = response.error {
        return Err(format!("get_public_key failed: {error}"));
    }
    let user_pubkey = match response.result {
        Some(ResponseResult::GetPublicKey(key)) => key.to_hex(),
        _ => return Err("phone returned no user public key".into()),
    };
    Ok((remote, user_pubkey))
}

fn pairing_request(
    ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    keys: &Keys,
    remote: PublicKey,
    request: NostrConnectRequest,
) -> Result<NostrConnectResponse, String> {
    let method = request.method();
    let message = NostrConnectMessage::request(&request);
    let request_id = message.id().to_owned();
    let event = NostrConnectEventBuilder::new(remote, message)
        .finalize(keys)
        .map_err(|e| e.to_string())?;
    ws.send(tungstenite::Message::text(
        serde_json::json!(["EVENT", event]).to_string(),
    ))
    .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        let text = match ws.read() {
            Ok(tungstenite::Message::Text(text)) => text,
            Ok(tungstenite::Message::Close(_)) => {
                return Err("relay closed during get_public_key".into());
            }
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if value.get(0).and_then(|v| v.as_str()) != Some("EVENT") {
            continue;
        }
        let Some(raw_event) = value.get(2) else {
            continue;
        };
        let Ok(event) = serde_json::from_value::<Event>(raw_event.clone()) else {
            continue;
        };
        if event.kind != Kind::NostrConnect || event.pubkey != remote || event.verify().is_err() {
            continue;
        }
        let Ok(plaintext) = keys.nip44_decrypt(&remote, &event.content) else {
            continue;
        };
        let Ok(message) = NostrConnectMessage::from_json(&plaintext) else {
            continue;
        };
        if message.id() == request_id && message.is_response() {
            return message.to_response(method).map_err(|e| e.to_string());
        }
    }
    Err("timed out waiting for get_public_key".into())
}

fn write_pairing(dir: &std::path::Path, relay: &str, remote_pubkey: &str) -> Result<(), String> {
    let path = dir.join("pkcs11-nip46-pairing.json");
    let temp = dir.join("pkcs11-nip46-pairing.json.tmp");
    let data = serde_json::to_vec(
        &serde_json::json!({"relay":relay, "remote_signer_pubkey":remote_pubkey}),
    )
    .map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    file.write_all(&data).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}

fn take_secret(request: &mut serde_json::Value, key: &str) -> Result<Zeroizing<String>, String> {
    let value = request
        .get_mut(key)
        .ok_or_else(|| format!("first-user request is missing {key}"))?;
    match std::mem::replace(value, serde_json::Value::Null) {
        serde_json::Value::String(value) => Ok(Zeroizing::new(value)),
        _ => Err(format!("first-user request field {key} must be a string")),
    }
}

fn create_first_user_account(
    username: &str,
    display_name: &str,
    identity_secret: &str,
    password: &str,
    pairing_enabled: bool,
) -> Result<(), String> {
    validate_username(username)?;
    if display_name.trim().is_empty()
        || display_name.chars().any(|character| character.is_control())
    {
        return Err("display name must not be empty or contain control characters".into());
    }
    if password.len() < 8 {
        return Err("use a passphrase of at least 8 characters".into());
    }
    if !list_homes()?.is_empty() {
        return Err("first-user setup only runs when systemd-homed has no accounts".into());
    }
    if PathBuf::from("/var/lib/evermore/first-user-enrolled").exists() {
        return Err("first-user setup has already completed".into());
    }

    let identity_secret = Zeroizing::new(identity_secret.to_owned());
    let secret_key = SecretKey::parse(identity_secret.trim())
        .map_err(|e| format!("invalid identity secret: {e}"))?;
    let keys = Keys::new(secret_key);
    let password_hash = hash_password(password)?;
    let token_pin = if pairing_enabled {
        ensure_pkcs11_pairing()?;
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|e| format!("generating PKCS#11 PIN: {e}"))?;
        let pin = Zeroizing::new(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>());
        bytes.zeroize();
        Some(pin)
    } else {
        None
    };
    let mut secret = serde_json::json!({ "password": [password] });
    if let Some(pin) = &token_pin {
        secret["tokenPin"] = serde_json::json!([pin.to_string()]);
    }
    let mut record = serde_json::json!({
        "userName": username,
        "realName": display_name,
        "homeDirectory": format!("/home/{username}"),
        "shell": "/bin/bash",
        "storage": "luks",
        "privileged": { "hashedPassword": [password_hash] },
        "evermore": {
            "npub": keys.public_key().to_bech32().map_err(|e| e.to_string())?,
            "pubkeyHex": keys.public_key().to_hex(),
        },
        "secret": secret,
    });
    if token_pin.is_some() {
        record["pkcs11TokenUri"] = serde_json::json!(["pkcs11:token=evermore-nip46;id=%01"]);
    }
    let mut record_json =
        Zeroizing::new(serde_json::to_string(&record).map_err(|e| e.to_string())?);
    let create_result = system_connection().and_then(|connection| {
        connection
            .call_method(
                Some(DESTINATION),
                PATH,
                Some(INTERFACE),
                "CreateHome",
                &(record_json.as_str(),),
            )
            .map(|_| ())
            .map_err(|e| format!("CreateHome failed: {e}"))
    });
    record_json.zeroize();
    if let Some(secret) = record.get_mut("secret")
        && let Some(passwords) = secret
            .get_mut("password")
            .and_then(serde_json::Value::as_array_mut)
    {
        for value in passwords.iter_mut() {
            if let serde_json::Value::String(value) = value {
                value.zeroize();
            }
        }
        passwords.clear();
        if let Some(pins) = secret
            .get_mut("tokenPin")
            .and_then(serde_json::Value::as_array_mut)
        {
            for pin in pins.iter_mut() {
                if let serde_json::Value::String(pin) = pin {
                    pin.zeroize();
                }
            }
            pins.clear();
        }
    }
    create_result?;
    activate_with_password(username, password)?;

    let (uid, gid) = list_homes()?
        .into_iter()
        .find(|home| home.0 == username)
        .map(|home| (home.1, home.3))
        .ok_or_else(|| "created home is missing from homed's user list".to_string())?;
    install_identity_secret(username, uid, gid, &keys)?;
    enable_identity_services(username, uid, gid)?;
    write_greeter_profile(&keys.public_key().to_hex(), display_name)?;
    write_first_user_marker()
}

/// Enable the per-identity user services needed by the desktop control plane.
/// The user manager starts these links when the newly enrolled user logs in.
fn enable_identity_services(username: &str, uid: u32, gid: u32) -> Result<(), String> {
    let home = list_homes()?
        .into_iter()
        .find(|entry| entry.0 == username)
        .map(|entry| PathBuf::from(entry.4))
        .ok_or_else(|| "created home is missing from homed's user list".to_string())?;
    let wants = home.join(".config/systemd/user/default.target.wants");
    std::fs::create_dir_all(&wants)
        .map_err(|e| format!("creating user service directory {}: {e}", wants.display()))?;
    for directory in [
        home.join(".config"),
        home.join(".config/systemd"),
        home.join(".config/systemd/user"),
        wants.clone(),
    ] {
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("setting permissions on {}: {e}", directory.display()))?;
        chown(&directory, uid, gid)?;
    }
    for unit in ["evermore-settingsd", "evermore-operationsd"] {
        let link = wants.join(format!("{unit}@{username}.service"));
        let target = PathBuf::from(format!("../{unit}@.service"));
        match std::fs::read_link(&link) {
            Ok(existing) if existing == target => {}
            Ok(_) => {
                return Err(format!(
                    "refusing to replace unexpected user service link {}",
                    link.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::os::unix::fs::symlink(&target, &link)
                    .map_err(|e| format!("enabling {unit} for {username}: {e}"))?;
            }
            Err(error) => {
                return Err(format!(
                    "checking user service link {}: {error}",
                    link.display()
                ));
            }
        }
    }
    Ok(())
}

fn ensure_pkcs11_pairing() -> Result<(), String> {
    let pairing = PathBuf::from("/var/lib/evermore/pkcs11-nip46-pairing.json");
    if !pairing.is_file() {
        return Err("phone pairing is missing; pair the phone again".into());
    }
    ensure_pkcs11_module()
}

fn ensure_pkcs11_module() -> Result<(), String> {
    let module = PathBuf::from("/usr/lib/pkcs11/evermore-pkcs11.so");
    let registration = PathBuf::from("/usr/share/p11-kit/modules/evermore-pkcs11.module");
    if !module.is_file() || !registration.is_file() {
        return Err("the Evermore PKCS#11 module is not installed and registered; install it before enabling phone unlock".into());
    }
    Ok(())
}

fn first_user() -> Result<(), String> {
    if !nix_is_root() {
        return Err("first-user must run as root from the first-boot enrollment service".into());
    }
    let existing = list_homes()?;
    if !existing.is_empty() {
        let names = existing
            .iter()
            .map(|home| home.0.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "homed already contains {names}; first-user will not modify existing homes. If this is an interrupted first setup, inspect the record and remove only that incomplete account with `homectl remove USER`, then restart this service."
        ));
    }

    println!("Evermore first-user setup\n");
    let username = read_line("Account name (lowercase letters, numbers, _ or -): ")?;
    validate_username(&username)?;
    let display_name = read_line("Display name: ")?;
    if display_name.trim().is_empty()
        || display_name.chars().any(|character| character.is_control())
    {
        return Err("display name must not be empty or contain control characters".into());
    }

    let mode = read_line("Create a new identity or import an nsec? [new/import]: ")?;
    let keys = match mode.as_str() {
        "new" => Keys::new(SecretKey::generate()),
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
    let password_hash = hash_password(&password)?;
    let mut record = serde_json::json!({
        "userName": &username,
        "realName": &display_name,
        "homeDirectory": format!("/home/{username}"),
        "shell": "/bin/bash",
        "storage": "luks",
        "privileged": { "hashedPassword": [password_hash] },
        "evermore": {
            "npub": pubkey.to_bech32().map_err(|e| e.to_string())?,
            "pubkeyHex": pubkey.to_hex(),
        },
        "secret": { "password": [password.as_str()] },
    });
    let mut record_json =
        Zeroizing::new(serde_json::to_string(&record).map_err(|e| e.to_string())?);
    let create_result = system_connection().and_then(|connection| {
        connection
            .call_method(
                Some(DESTINATION),
                PATH,
                Some(INTERFACE),
                "CreateHome",
                &(record_json.as_str(),),
            )
            .map(|_| ())
            .map_err(|e| format!("CreateHome failed: {e}"))
    });
    record_json.zeroize();
    if let Some(secret) = record.get_mut("secret")
        && let Some(passwords) = secret
            .get_mut("password")
            .and_then(serde_json::Value::as_array_mut)
    {
        for value in passwords.iter_mut() {
            if let serde_json::Value::String(value) = value {
                value.zeroize();
            }
        }
        passwords.clear();
    }
    create_result?;
    activate_with_password(&username, &password)?;

    let (uid, gid) = list_homes()?
        .into_iter()
        .find(|home| home.0 == username)
        .map(|home| (home.1, home.3))
        .ok_or_else(|| "created home is missing from homed's user list".to_string())?;
    install_identity_secret(&username, uid, gid, &keys)?;
    enable_identity_services(&username, uid, gid)?;
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
    println!("First-user enrollment complete. The greeter will start now.");
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

fn hash_password(password: &str) -> Result<String, String> {
    let mut child = std::process::Command::new("openssl")
        .args(["passwd", "-6", "-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting OpenSSL password hashing: {e} (install openssl)"))?;
    {
        let mut input = child.stdin.take().ok_or("OpenSSL stdin is unavailable")?;
        input
            .write_all(password.as_bytes())
            .and_then(|_| input.write_all(b"\n"))
            .map_err(|e| format!("sending passphrase to OpenSSL: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("waiting for OpenSSL: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "OpenSSL password hashing failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let hash = String::from_utf8(output.stdout)
        .map_err(|e| format!("OpenSSL returned invalid UTF-8: {e}"))?;
    let hash = hash.trim();
    if !hash.starts_with("$6$") {
        return Err("OpenSSL returned an unexpected password hash format".into());
    }
    Ok(hash.to_string())
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

#[cfg(test)]
mod nip46_pairing_tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    use tungstenite::Message;
    use tungstenite::stream::MaybeTlsStream;

    fn start_relay() -> (String, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let peers: Arc<Mutex<Vec<mpsc::Sender<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let thread_stop = stop.clone();
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let peers = peers.clone();
                        let stop = thread_stop.clone();
                        std::thread::spawn(move || relay_peer(stream, peers, stop));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        (format!("ws://127.0.0.1:{port}"), stop)
    }

    fn relay_peer(
        stream: TcpStream,
        peers: Arc<Mutex<Vec<mpsc::Sender<String>>>>,
        stop: Arc<AtomicBool>,
    ) {
        let Ok(mut ws) = tungstenite::accept(stream) else {
            return;
        };
        let _ = ws
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(50)));
        let (tx, rx) = mpsc::channel::<String>();
        peers.lock().unwrap().push(tx);
        while !stop.load(Ordering::Relaxed) {
            match ws.read() {
                Ok(Message::Text(text)) => {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        match value.get(0).and_then(|v| v.as_str()) {
                            Some("REQ") => {
                                if let Some(id) = value.get(1).and_then(|v| v.as_str()) {
                                    let _ = ws.send(Message::text(
                                        serde_json::json!(["EOSE", id]).to_string(),
                                    ));
                                }
                            }
                            Some("EVENT") => {
                                if let Some(event) = value.get(1) {
                                    let id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                    let _ = ws.send(Message::text(
                                        serde_json::json!(["OK", id, true, ""]).to_string(),
                                    ));
                                    let broadcast =
                                        serde_json::json!(["EVENT", "pairing-test", event])
                                            .to_string();
                                    for peer in peers.lock().unwrap().iter() {
                                        let _ = peer.send(broadcast.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
            while let Ok(message) = rx.try_recv() {
                if ws.send(Message::text(message)).is_err() {
                    return;
                }
            }
        }
    }

    fn mock_signer(relay: &str, keys: Keys, uri_rx: mpsc::Receiver<String>) {
        let uri = uri_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(uri.contains("perms=get_public_key%2Csign_event%3A31337"));
        let (client, secret) = match NostrConnectUri::parse(uri).unwrap() {
            NostrConnectUri::Client {
                public_key, secret, ..
            } => (public_key, secret),
            _ => panic!("expected client NIP-46 URI"),
        };
        let (mut ws, _) = tungstenite::connect(relay).unwrap();
        if let MaybeTlsStream::Plain(tcp) = ws.get_ref() {
            let _ = tcp.set_read_timeout(Some(Duration::from_millis(100)));
        }
        let filter = Filter::new()
            .kind(Kind::NostrConnect)
            .author(client)
            .pubkey(keys.public_key());
        ws.send(Message::text(
            serde_json::json!(["REQ", "mock-phone", filter]).to_string(),
        ))
        .unwrap();
        let connect_response = NostrConnectMessage::Response {
            id: "phone-connect".into(),
            result: Some(secret),
            error: None,
        };
        let event = NostrConnectEventBuilder::new(client, connect_response)
            .finalize(&keys)
            .unwrap();
        ws.send(Message::text(
            serde_json::json!(["EVENT", event]).to_string(),
        ))
        .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let text = match ws.read() {
                Ok(Message::Text(text)) => text,
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Ok(_) => continue,
                Err(e) => panic!("mock signer relay read failed: {e}"),
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if value.get(0).and_then(|v| v.as_str()) != Some("EVENT") {
                continue;
            }
            let Ok(event) = serde_json::from_value::<Event>(value[2].clone()) else {
                continue;
            };
            if event.kind != Kind::NostrConnect || event.pubkey != client {
                continue;
            }
            let Ok(plaintext) = keys.nip44_decrypt(&client, &event.content) else {
                continue;
            };
            let message = NostrConnectMessage::from_json(&plaintext).unwrap();
            let id = message.id().to_owned();
            assert!(matches!(
                message.to_request().unwrap(),
                NostrConnectRequest::GetPublicKey
            ));
            let response = NostrConnectMessage::response(
                id,
                NostrConnectResponse::with_result(ResponseResult::GetPublicKey(keys.public_key())),
            );
            let reply = NostrConnectEventBuilder::new(client, response)
                .finalize(&keys)
                .unwrap();
            ws.send(Message::text(
                serde_json::json!(["EVENT", reply]).to_string(),
            ))
            .unwrap();
            return;
        }
        panic!("pairing client did not request get_public_key");
    }

    #[test]
    fn connection_qr_pairing_completes_with_mock_signer() {
        let (relay, stop) = start_relay();
        let client_keys = Keys::generate();
        let signer_keys = Keys::generate();
        let expected_signer = signer_keys.public_key();
        let (uri_tx, uri_rx) = mpsc::channel();
        let signer_relay = relay.clone();
        let signer_keys_for_thread = signer_keys.clone();
        let signer_thread =
            std::thread::spawn(move || mock_signer(&signer_relay, signer_keys_for_thread, uri_rx));
        let result = pair_over_relay(&relay, &client_keys, |uri| {
            uri_tx.send(uri.to_owned()).map_err(|e| e.to_string())
        });
        signer_thread.join().unwrap();
        stop.store(true, Ordering::Relaxed);
        let (remote, user) = result.expect("mock NIP-46 pairing failed");
        assert_eq!(remote, expected_signer);
        assert_eq!(user, expected_signer.to_hex());
    }
}
