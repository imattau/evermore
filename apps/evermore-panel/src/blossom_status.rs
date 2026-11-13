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
    action_status: Option<String>,
}

impl State {
    fn refresh(&mut self, identities: &[String]) {
        self.statuses.clear();
        for identity in identities {
            self.statuses
                .insert(identity.clone(), read_status(identity));
        }
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
    ui.weak("If the service is missing or failed, configure BLOSSOM_URL in its user-unit override before enabling or restarting it.");
    if let Some(status) = &state.action_status {
        ui.label(status);
    }
}
