//! Local system settings and desktop entry points.

use eframe::egui;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

#[derive(Default)]
pub struct SystemSettingsState {
    loaded: bool,
    locales: Vec<String>,
    timezones: Vec<String>,
    homed_accounts: Option<String>,
    locale: String,
    local_locale: String,
    synced_locale: Option<String>,
    timezone: String,
    local_timezone: String,
    synced_timezone: Option<String>,
    local_ntp: Option<bool>,
    synced_ntp: Option<bool>,
    ntp_enabled: bool,
    timezone_filter: String,
    status: Option<String>,
    error: Option<String>,
    relay_error: Option<String>,
    action_rx: Option<Receiver<Result<String, String>>>,
    pending_session_action: Option<&'static str>,
}

fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let result = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Couldn't run {program}: {error}"))?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

fn parse_system_bool(value: String) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "1" => Some(true),
        "no" | "false" | "0" => Some(false),
        _ => None,
    }
}

fn refresh(state: &mut SystemSettingsState) {
    state.locales = output("localectl", &["list-locales"])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    state.timezones = output("timedatectl", &["list-timezones"])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    state.homed_accounts = output("homectl", &["list", "--no-pager"])
        .ok()
        .map(|text| text.chars().take(1600).collect());
    state.local_locale = output("localectl", &["--no-pager", "status"])
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("System Locale:")
                    .map(str::trim)
                    .and_then(|value| value.strip_prefix("LANG="))
                    .map(str::to_owned)
            })
        })
        .unwrap_or_default();
    state.local_timezone =
        output("timedatectl", &["show", "--property=Timezone", "--value"]).unwrap_or_default();
    state.local_ntp = output("timedatectl", &["show", "--property=NTP", "--value"])
        .ok()
        .and_then(parse_system_bool);
    match super::settings_client::get_domain("system-preferences") {
        Ok(preferences) => {
            state.relay_error = None;
            state.synced_locale = preferences
                .get("locale")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            state.synced_timezone = preferences
                .get("timezone")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            state.synced_ntp = preferences
                .get("automatic_time_sync")
                .and_then(serde_json::Value::as_bool);
        }
        Err(error) => {
            state.relay_error = Some(error);
            state.synced_locale = None;
            state.synced_timezone = None;
            state.synced_ntp = None;
        }
    }
    state.locale = state
        .synced_locale
        .as_ref()
        .filter(|locale| state.locales.contains(locale))
        .cloned()
        .unwrap_or_else(|| state.local_locale.clone());
    state.timezone = state
        .synced_timezone
        .as_ref()
        .filter(|timezone| state.timezones.contains(timezone))
        .cloned()
        .unwrap_or_else(|| state.local_timezone.clone());
    state.ntp_enabled = state.synced_ntp.or(state.local_ntp).unwrap_or(false);
    state.loaded = true;
}

fn start_synced_authorized(
    state: &mut SystemSettingsState,
    key: &'static str,
    value: serde_json::Value,
    args: Vec<String>,
    success: String,
) {
    if state.action_rx.is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    state.action_rx = Some(rx);
    std::thread::spawn(move || {
        let result = super::settings_client::set(
            "system-preferences",
            key,
            value,
        )
        .map_err(|error| format!("Couldn't sync this preference to your Evermore identity: {error}"))
        .and_then(|()| {
            Command::new("pkexec")
                .args(args)
                .stdin(Stdio::null())
                .output()
                .map_err(|error| {
                    format!("Preference saved for identity sync, but couldn't open the system authorization prompt: {error}")
                })
                .and_then(|output| {
                    if output.status.success() {
                        Ok(success)
                    } else {
                        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                        Err(if message.is_empty() {
                            "Preference saved for identity sync, but the local system change was cancelled or denied.".into()
                        } else {
                            format!("Preference saved for identity sync, but local system change failed: {message}")
                        })
                    }
                })
        });
        let _ = tx.send(result);
    });
}

fn launch(program: &str) -> Result<(), String> {
    Command::new(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Couldn't open {program}: {error}"))
}

pub fn ui(ui: &mut egui::Ui, state: &mut SystemSettingsState) {
    if !state.loaded {
        refresh(state);
    }
    let action_result = state.action_rx.as_ref().map(Receiver::try_recv);
    if let Some(action_result) = action_result {
        match action_result {
            Ok(Ok(message)) => {
                state.status = Some(message);
                state.error = None;
                state.action_rx = None;
                refresh(state);
            }
            Ok(Err(error)) => {
                state.error = Some(error);
                state.status = None;
                state.action_rx = None;
                refresh(state);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                state.error = Some("The system settings operation stopped unexpectedly.".into());
                state.action_rx = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    ui.heading("System");
    ui.weak("Locale, time-zone, and automatic clock preferences sync through your Evermore identity; OS changes are applied on this device after confirmation.");
    ui.add_space(10.0);

    egui::CollapsingHeader::new("Users and session")
        .default_open(true)
        .show(ui, |ui| {
            let username = std::env::var("USER").unwrap_or_else(|_| "Unknown".into());
            ui.label(format!("Signed in as {username}"));
            ui.label(format!(
                "Evermore identity: {}",
                std::env::var("EVERMORE_IDENTITY").unwrap_or_else(|_| "shown in Identity".into())
            ));
            ui.weak("Linux accounts and Evermore identities are separate. First-user enrollment is handled by the greeter.");
            if let Some(accounts) = &state.homed_accounts {
                ui.label("Home accounts on this device");
                ui.monospace(accounts);
            } else {
                ui.weak("The system home-account service is unavailable; account list could not be loaded.");
            }
            ui.horizontal(|ui| {
                if ui.button("Lock screen").clicked() {
                    if let Err(error) = launch("swaylock") {
                        state.error = Some(error);
                    }
                }
                if ui.button("Sign out…").clicked() {
                    state.pending_session_action = Some("sign out");
                }
            });
            ui.weak("This prototype does not yet provide graphical management of additional Linux accounts.");
        });

    egui::CollapsingHeader::new("Language and regional format")
        .default_open(true)
        .show(ui, |ui| {
            ui.label("Operating system locale (language and regional formats)");
            if state.locales.is_empty() {
                ui.weak("No generated locales were found. Generate a locale using the system's locale setup before selecting it.");
            } else {
                egui::ComboBox::from_id_salt("system-locale")
                    .selected_text(if state.locale.is_empty() { "Unknown" } else { &state.locale })
                    .show_ui(ui, |ui| {
                        for locale in &state.locales {
                            ui.selectable_value(&mut state.locale, locale.clone(), locale);
                        }
                    });
                if ui
                    .add_enabled(
                        state.action_rx.is_none() && !state.locale.is_empty(),
                        egui::Button::new("Apply system locale…"),
                    )
                    .clicked()
                {
                    state.pending_session_action = Some("apply locale");
                }
                if !state.locale.is_empty() {
                    let preview = Command::new("date")
                        .env("LC_TIME", &state.locale)
                        .arg("+%A, %d %B %Y · %H:%M")
                        .output()
                        .ok()
                        .filter(|result| result.status.success())
                        .map(|result| String::from_utf8_lossy(&result.stdout).trim().to_owned())
                        .unwrap_or_else(|| "Preview unavailable".into());
                    ui.label(format!("Date and time preview: {preview}"));
                }
            }
            ui.weak("The system locale affects compatible desktop apps for all users after sign-out. Evermore's interface is currently English-only.");
            if let Some(error) = &state.relay_error {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Couldn't load identity-synced preferences: {error}"),
                );
            }
            ui.weak(format!(
                "This device: {} · identity preference: {}",
                if state.local_locale.is_empty() { "unknown" } else { &state.local_locale },
                state.synced_locale.as_deref().unwrap_or("not set")
            ));
        });

    egui::CollapsingHeader::new("Date and time")
        .default_open(true)
        .show(ui, |ui| {
            let now = output("date", &["+%A, %d %B %Y · %H:%M:%S %Z"]).unwrap_or_else(|_| "Unavailable".into());
            ui.label(format!("Local time: {now}"));
            ui.horizontal(|ui| {
                ui.label("Time zone");
                ui.add(
                    egui::TextEdit::singleline(&mut state.timezone_filter)
                        .desired_width(190.0)
                        .hint_text("Search, e.g. Australia/Melbourne"),
                );
            });
            let matches: Vec<_> = state
                .timezones
                .iter()
                .filter(|zone| state.timezone_filter.is_empty() || zone.to_lowercase().contains(&state.timezone_filter.to_lowercase()))
                .take(12)
                .cloned()
                .collect();
            egui::ComboBox::from_id_salt("system-timezone")
                .selected_text(if state.timezone.is_empty() { "Unknown" } else { &state.timezone })
                .show_ui(ui, |ui| {
                    for zone in matches {
                        ui.selectable_value(&mut state.timezone, zone.clone(), zone);
                    }
                });
            if ui
                .add_enabled(state.action_rx.is_none() && !state.timezone.is_empty(), egui::Button::new("Apply time zone…"))
                .clicked()
            {
                state.pending_session_action = Some("apply time zone");
            }
            let synchronized = output(
                "timedatectl",
                &["show", "--property=NTPSynchronized", "--value"],
            )
            .unwrap_or_else(|_| "unknown".into());
            if ui
                .checkbox(&mut state.ntp_enabled, "Automatic time synchronization")
                .changed()
            {
                state.pending_session_action = Some("apply ntp");
            }
            ui.weak(format!(
                "This device: {} · identity preference: {} · currently synchronized: {synchronized}",
                state.local_ntp.map(|enabled| if enabled { "on" } else { "off" }).unwrap_or("unknown"),
                state.synced_ntp.map(|enabled| if enabled { "on" } else { "off" }).unwrap_or("not set")
            ));
            ui.weak("Quiet-hours schedules use this device's local clock. Changing the time zone changes how their hours are interpreted.");
            ui.weak(format!(
                "This device: {} · identity preference: {}",
                if state.local_timezone.is_empty() { "unknown" } else { &state.local_timezone },
                state.synced_timezone.as_deref().unwrap_or("not set")
            ));
        });

    egui::CollapsingHeader::new("Power and devices")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, command) in [
                    ("Network", "nm-connection-editor"),
                    ("Audio", "pavucontrol"),
                    ("Display", "wdisplays"),
                    ("Bluetooth", "blueman-manager"),
                ] {
                    if ui.button(label).clicked() {
                        if let Err(error) = launch(command) {
                            state.error = Some(error);
                        }
                    }
                }
            });
            ui.weak("Battery status appears in the desktop bar. Display, network, audio, and Bluetooth open their upstream system tools.");
        });

    if let Some(message) = &state.status {
        ui.colored_label(egui::Color32::from_rgb(95, 190, 140), message);
    }
    if let Some(error) = &state.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
        if ui.small_button("Refresh system status").clicked() {
            refresh(state);
            state.error = None;
        }
    }
    if ui
        .small_button("Refresh system and synced preferences")
        .clicked()
    {
        refresh(state);
        state.error = None;
    }
    if state.action_rx.is_some() {
        ui.weak("Waiting for system authorization…");
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(150));
    }

    if let Some(action) = state.pending_session_action {
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Confirm system change")
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                match action {
                    "sign out" => { ui.label("Sign out of this desktop session?"); }
                    "apply locale" => { ui.label(format!("Apply {} as the system locale? You must sign out for all apps to pick it up.", state.locale)); }
                    "apply time zone" => { ui.label(format!("Set the device time zone to {}?", state.timezone)); }
                    "apply ntp" => { ui.label(format!("Turn automatic time synchronization {} on this device?", if state.ntp_enabled { "on" } else { "off" })); }
                    _ => {}
                }
                ui.horizontal(|ui| {
                    if ui.button("Confirm").clicked() { confirm = true; }
                    if ui.button("Cancel").clicked() { cancel = true; }
                });
            });
        if confirm {
            state.pending_session_action = None;
            match action {
                "sign out" => {
                    if let Ok(session) = std::env::var("XDG_SESSION_ID") {
                        if let Err(error) = Command::new("loginctl")
                            .args(["terminate-session", &session])
                            .spawn()
                        {
                            state.error = Some(format!("Couldn't sign out: {error}"));
                        }
                    } else {
                        state.error = Some("The current session ID is unavailable; use the desktop's sign-out action.".into());
                    }
                }
                "apply locale" => {
                    if state.locales.contains(&state.locale) {
                        let locale = state.locale.clone();
                        start_synced_authorized(
                            state,
                            "locale",
                            serde_json::Value::String(locale.clone()),
                            vec![
                                "localectl".into(),
                                "set-locale".into(),
                                format!("LANG={locale}"),
                            ],
                            "System locale updated and preference saved for identity sync. Sign out and back in to apply it to apps.".into(),
                        );
                    } else {
                        state.error = Some("Choose a locale installed on this device.".into());
                    }
                }
                "apply time zone" => {
                    if state.timezones.contains(&state.timezone) {
                        let timezone = state.timezone.clone();
                        start_synced_authorized(
                            state,
                            "timezone",
                            serde_json::Value::String(timezone.clone()),
                            vec!["timedatectl".into(), "set-timezone".into(), timezone],
                            "Time zone updated and preference saved for identity sync.".into(),
                        );
                    } else {
                        state.error = Some("Choose a time zone from the installed list.".into());
                    }
                }
                "apply ntp" => {
                    let value = state.ntp_enabled;
                    start_synced_authorized(
                        state,
                        "automatic_time_sync",
                        serde_json::Value::Bool(value),
                        vec![
                            "timedatectl".into(),
                            "set-ntp".into(),
                            if value { "true".into() } else { "false".into() },
                        ],
                        format!(
                            "Automatic time synchronization turned {} and preference saved for identity sync.",
                            if value { "on" } else { "off" }
                        ),
                    );
                }
                _ => {}
            }
        } else if cancel {
            state.pending_session_action = None;
            state.ntp_enabled = state.synced_ntp.or(state.local_ntp).unwrap_or(false);
        }
    }
}
