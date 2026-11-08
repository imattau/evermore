//! npack has no socket -- unlike the other tabs' clients, this shells out
//! to the `evermore-npack` CLI (`--json` for machine-readable output) and
//! parses its stdout. Simpler than adding a fourth daemon for something
//! that doesn't need live push updates or shared cached state.

use serde::Deserialize;
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
pub struct InstalledPackage {
    pub id: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AvailablePackage {
    pub id: String,
    pub name: String,
    pub description: String,
    pub publisher_pubkey: String,
    pub version: Option<String>,
}

pub fn installed() -> Result<Vec<InstalledPackage>, String> {
    run_json(&["installed", "--json"])
}

pub fn trusted(identity: &str) -> Result<Vec<String>, String> {
    run_json(&["trusted", "--identity", identity, "--json"])
}

pub fn available(identity: &str) -> Result<Vec<AvailablePackage>, String> {
    run_json(&["available", "--identity", identity, "--json"])
}

pub fn trust(identity: &str, pubkey: &str) -> Result<(), String> {
    run(&["trust", pubkey, "--identity", identity])
}

pub fn untrust(identity: &str, pubkey: &str) -> Result<(), String> {
    run(&["untrust", pubkey, "--identity", identity])
}

pub fn install(identity: &str, blossom_url: &str, id: &str) -> Result<(), String> {
    run(&[
        "install",
        id,
        "--identity",
        identity,
        "--blossom-url",
        blossom_url,
    ])
}

pub fn update(identity: &str, blossom_url: &str, id: &str) -> Result<(), String> {
    run(&[
        "update",
        id,
        "--identity",
        identity,
        "--blossom-url",
        blossom_url,
    ])
}

pub fn remove(id: &str) -> Result<(), String> {
    run(&["remove", id])
}

fn run(args: &[&str]) -> Result<(), String> {
    let output = Command::new("evermore-npack")
        .args(args)
        .output()
        .map_err(|e| format!("couldn't run evermore-npack ({e}) -- is it on PATH?"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(())
}

fn run_json<T: for<'de> Deserialize<'de>>(args: &[&str]) -> Result<T, String> {
    let output = Command::new("evermore-npack")
        .args(args)
        .output()
        .map_err(|e| format!("couldn't run evermore-npack ({e}) -- is it on PATH?"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).map_err(|e| format!("parsing evermore-npack output: {e}"))
}
