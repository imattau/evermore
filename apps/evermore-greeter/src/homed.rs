//! Enumerates provisioned users straight from `systemd-homed`'s real
//! management API (`org.freedesktop.home1.Manager` over the system
//! D-Bus -- the same interface `evermore-enroll` uses, verified
//! directly against a running VM rather than assumed). Both methods
//! used here are `SD_BUS_VTABLE_UNPRIVILEGED`, so the dedicated
//! `greeter` system user this binary runs as needs no special
//! permissions to call them.

use zbus::blocking::Connection;

const DESTINATION: &str = "org.freedesktop.home1";
const PATH: &str = "/org/freedesktop/home1";
const INTERFACE: &str = "org.freedesktop.home1.Manager";

pub struct HomedUser {
    pub user_name: String,
    pub state: String,
    /// `evermore.pubkeyHex` from the user's own homed record, if the
    /// account was provisioned with `evermore-enroll create
    /// --identity-pubkey ...` (see PROJECT_IDEA.md's Phase 5.5 --
    /// "the pubkey is the identity"). `None` for an ordinary,
    /// non-Evermore homed account -- still listed, just with no
    /// profile to look up.
    pub pubkey_hex: Option<String>,
    /// True when the account record includes Evermore's PKCS#11 unlock
    /// factor. The greeter uses this to expose the explicit phone option.
    pub phone_unlock_available: bool,
}

/// `ListHomes`'s tuple order (`a(susussso)`), confirmed by reading
/// `method_list_homes` in systemd's own source rather than guessed:
/// `(userName, uid, state, gid, realName, homeDirectory, shell, path)`.
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

pub fn list_users() -> Result<Vec<HomedUser>, String> {
    let connection =
        Connection::system().map_err(|e| format!("connecting to the system bus: {e}"))?;

    let reply = connection
        .call_method(Some(DESTINATION), PATH, Some(INTERFACE), "ListHomes", &())
        .map_err(|e| format!("ListHomes failed: {e}"))?;
    let entries: Vec<ListHomesEntry> = reply.body().deserialize().map_err(|e| e.to_string())?;

    let mut users = Vec::with_capacity(entries.len());
    for (user_name, _uid, state, ..) in entries {
        let (pubkey_hex, phone_unlock_available) = get_user_record_details(&connection, &user_name);
        users.push(HomedUser {
            user_name,
            state,
            pubkey_hex,
            phone_unlock_available,
        });
    }
    Ok(users)
}

fn get_user_record_details(connection: &Connection, user_name: &str) -> (Option<String>, bool) {
    let reply = connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "GetUserRecordByName",
            &(user_name,),
        )
        .ok();
    let Some(reply) = reply else {
        return (None, false);
    };
    let (user_record_json, _incomplete, _path): (String, bool, zbus::zvariant::OwnedObjectPath) =
        match reply.body().deserialize() {
            Ok(record) => record,
            Err(_) => return (None, false),
        };
    let Ok(record) = serde_json::from_str::<serde_json::Value>(&user_record_json) else {
        return (None, false);
    };
    let pubkey = record
        .get("evermore")
        .and_then(|evermore| evermore.get("pubkeyHex"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let phone_unlock_available = record
        .get("pkcs11TokenUri")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|uris| !uris.is_empty());
    (pubkey, phone_unlock_available)
}
