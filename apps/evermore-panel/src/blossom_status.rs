//! Read-only health view for the per-identity Blossom sync service.

use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Clone)]
struct UnitStatus {
    active: String,
    sub: String,
    load: String,
}

#[derive(Default)]
pub(crate) struct State {
    statuses: HashMap<String, Result<UnitStatus, String>>,
    recent_failures: HashMap<String, Result<Vec<String>, String>>,
    recent_conflicts: HashMap<String, Result<Vec<String>, String>>,
    storage: Option<Result<(u64, u64), String>>,
    action_status: Option<String>,
}

impl State {
    fn refresh(&mut self, identities: &[String]) {
        self.statuses.clear();
        self.recent_failures.clear();
        self.recent_conflicts.clear();
        self.storage = Some(local_storage_usage());
        for identity in identities {
            self.statuses
                .insert(identity.clone(), read_status(identity));
            self.recent_failures
                .insert(identity.clone(), read_recent_failures(identity));
            self.recent_conflicts
                .insert(identity.clone(), read_recent_conflicts(identity));
        }
    }
}

fn local_storage_usage() -> Result<(u64, u64), String> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())?;
    let directory = home.join("Nostr");
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| format!("couldn't read {}: {error}", directory.display()))?;
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("couldn't read sync directory entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("couldn't inspect sync directory entry: {error}"))?;
        if file_type.is_file() {
            files = files.saturating_add(1);
            bytes = bytes.saturating_add(
                entry
                    .metadata()
                    .map_err(|error| format!("couldn't inspect synced file: {error}"))?
                    .len(),
            );
        }
    }
    Ok((files, bytes))
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn unit_name(identity: &str) -> Result<String, String> {
    let escaped = Command::new("systemd-escape")
        .args(["--template=evermore-blossom-fs@.service", identity])
        .output()
        .map_err(|error| format!("couldn't start systemd-escape: {error}"))?;
    if !escaped.status.success() {
        return Err(String::from_utf8_lossy(&escaped.stderr).trim().to_owned());
    }
    let unit = String::from_utf8_lossy(&escaped.stdout).trim().to_owned();
    if unit.is_empty() || !unit.starts_with("evermore-blossom-fs@") {
        return Err("couldn't resolve this identity's sync service name".into());
    }
    Ok(unit)
}

fn read_status(identity: &str) -> Result<UnitStatus, String> {
    let unit = unit_name(identity)?;
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            "--no-pager",
            "--property=LoadState,ActiveState,SubState",
            &unit,
        ])
        .output()
        .map_err(|error| format!("couldn't query systemd: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let mut load = "unknown".to_owned();
    let mut active = "unknown".to_owned();
    let mut sub = "unknown".to_owned();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(value) = line.strip_prefix("LoadState=") {
            load = value.to_owned();
        } else if let Some(value) = line.strip_prefix("ActiveState=") {
            active = value.to_owned();
        } else if let Some(value) = line.strip_prefix("SubState=") {
            sub = value.to_owned();
        }
    }
    Ok(UnitStatus { active, sub, load })
}

fn read_recent_failures(identity: &str) -> Result<Vec<String>, String> {
    let unit = unit_name(identity)?;
    let output = Command::new("journalctl")
        .args([
            "--user",
            "-u",
            &unit,
            "--since",
            "24 hours ago",
            "--no-pager",
            "--output=cat",
            "-n",
            "200",
        ])
        .output()
        .map_err(|error| format!("couldn't read service journal: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let failures: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            [
                "couldn't upload ",
                "couldn't download ",
                "couldn't write ",
                "couldn't remove synced deletion ",
                "couldn't publish index: ",
                "couldn't sign index event: ",
            ]
            .iter()
            .any(|marker| line.contains(marker))
                || line.contains("failed its SHA-256 check")
        })
        .map(|line| {
            let line = line.trim();
            if line.chars().count() > 300 {
                format!("{}…", line.chars().take(300).collect::<String>())
            } else {
                line.to_owned()
            }
        })
        .collect();
    Ok(failures.into_iter().rev().take(5).collect())
}

fn read_recent_conflicts(identity: &str) -> Result<Vec<String>, String> {
    let unit = unit_name(identity)?;
    let output = Command::new("journalctl")
        .args([
            "--user",
            "-u",
            &unit,
            "--since",
            "24 hours ago",
            "--no-pager",
            "--output=cat",
            "-n",
            "200",
        ])
        .output()
        .map_err(|error| format!("couldn't read service journal: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            line.contains("preserving local edit for ")
                || line.contains("preserving local deletion for ")
        })
        .map(str::to_owned)
        .rev()
        .take(5)
        .collect())
}

fn open_logs(identity: &str) -> Result<(), String> {
    let unit = unit_name(identity)?;
    let args = ["--user", "-u", &unit, "-n", "100", "--no-pager"];
    for (terminal, prefix) in [("foot", vec!["-e"]), ("xterm", vec!["-e"])] {
        match Command::new(terminal)
            .args(prefix)
            .arg("journalctl")
            .args(args)
            .spawn()
        {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("couldn't open service logs: {error}")),
        }
    }
    Err("no supported terminal was found (install foot or xterm)".into())
}

pub(crate) fn ui(ui: &mut egui::Ui, state: &mut State, identities: &[String]) {
    ui.horizontal(|ui| {
        ui.strong("Blossom file sync");
        if ui.button("Refresh service status").clicked() {
            state.refresh(identities);
        }
    });
    ui.weak("This reports whether each local sync service is running. It does not confirm that every file has uploaded or downloaded.");

    match &state.storage {
        Some(Ok((files, bytes))) => {
            ui.label(format!(
                "Local sync folder: {files} files, {}",
                human_size(*bytes)
            ));
        }
        Some(Err(error)) => {
            ui.weak(format!("Local sync folder usage unavailable: {error}"));
        }
        None => {}
    }

    if identities.is_empty() {
        ui.weak("Add or unlock an Evermore identity to inspect its sync service.");
        return;
    }
    if state.statuses.is_empty() {
        ui.weak("Refresh to check sync services for the identities on this device.");
        return;
    }
    for identity in identities {
        ui.horizontal(|ui| {
            ui.label(identity);
            if ui.small_button("View logs").clicked() {
                state.action_status = Some(match open_logs(identity) {
                    Ok(()) => format!("Opened recent logs for {identity}."),
                    Err(error) => error,
                });
            }
            match state.statuses.get(identity) {
                Some(Ok(status)) if status.load == "not-found" => {
                    ui.weak("not enabled");
                }
                Some(Ok(status)) => {
                    let text = format!("{} / {}", status.active, status.sub);
                    if status.active == "active" {
                        ui.label(text);
                    } else {
                        ui.colored_label(ui.visuals().error_fg_color, text);
                    }
                }
                Some(Err(error)) => {
                    ui.colored_label(ui.visuals().error_fg_color, "status unavailable");
                    ui.weak(error);
                }
                None => {
                    ui.weak("not checked");
                }
            }
        });
    }
    for identity in identities {
        match state.recent_failures.get(identity) {
            Some(Ok(failures)) if !failures.is_empty() => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Recent sync failures for {identity}"),
                );
                for failure in failures {
                    ui.weak(failure);
                }
            }
            Some(Err(error)) => {
                ui.weak(format!(
                    "Couldn't read recent sync errors for {identity}: {error}"
                ));
            }
            _ => {}
        }
    }
    for identity in identities {
        if let Some(Ok(conflicts)) = state.recent_conflicts.get(identity) {
            if !conflicts.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("Local files preserved during remote updates for {identity}"),
                );
                for conflict in conflicts {
                    ui.weak(conflict);
                }
            }
        }
    }
    ui.weak("Recent file upload/download failures and preserved local changes are read from the last 24 hours of each service journal.");
    ui.weak("If the service is missing or failed, configure BLOSSOM_URL in its user-unit override before enabling or restarting it.");
    if let Some(status) = &state.action_status {
        ui.label(status);
    }
}
