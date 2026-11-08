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
        let pubkey_hex = get_user_record_pubkey(&connection, &user_name);
        users.push(HomedUser {
            user_name,
            state,
            pubkey_hex,
        });
    }
    Ok(users)
}

fn get_user_record_pubkey(connection: &Connection, user_name: &str) -> Option<String> {
    let reply = connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "GetUserRecordByName",
            &(user_name,),
        )
        .ok()?;
    let (user_record_json, _incomplete, _path): (String, bool, zbus::zvariant::OwnedObjectPath) =
        reply.body().deserialize().ok()?;
    let record: serde_json::Value = serde_json::from_str(&user_record_json).ok()?;
    record
        .get("evermore")?
        .get("pubkeyHex")?
        .as_str()
        .map(str::to_string)
}
