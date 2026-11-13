//! Read-only access to the trusted publisher list for the panel's status view.

use std::process::Command;

pub fn trusted_publishers(identity: &str) -> Result<Vec<String>, String> {
    if identity.is_empty() {
        return Err("No active identity is available to read the trust list.".into());
    }
    let output = Command::new("evermore-npack")
        .args(["trusted", "--identity", identity, "--json"])
        .output()
        .map_err(|error| format!("starting evermore-npack: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(if stderr.trim().is_empty() {
            format!("evermore-npack exited with {}", output.status)
        } else {
            stderr.trim().to_owned()
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim())
        .map_err(|error| format!("invalid trusted-publisher response: {error}"))
}

pub fn set_publisher_trust(identity: &str, pubkey: &str, trusted: bool) -> Result<(), String> {
    let action = if trusted { "trust" } else { "untrust" };
    let output = Command::new("evermore-npack")
        .args([action, pubkey, "--identity", identity])
        .output()
        .map_err(|error| format!("starting evermore-npack: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(if stderr.trim().is_empty() {
        format!("evermore-npack {action} exited with {}", output.status)
    } else {
        stderr.trim().to_owned()
    })
}
