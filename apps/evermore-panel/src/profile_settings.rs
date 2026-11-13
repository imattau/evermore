//! NIP-01 profile metadata editor backed by the local control-plane relay.

use crate::{relay_client, signer_client::Identity};
use eframe::egui;
use nostr::prelude::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

#[derive(Default)]
pub struct State {
    selected: String,
    loaded_for: String,
    display_name: String,
    original_name: String,
    picture_url: Option<String>,
    loading: Option<(String, Receiver<Result<Profile, String>>)>,
    saving: Option<Receiver<Result<SaveResult, String>>>,
    pending_save: Option<String>,
    error: Option<String>,
    status: Option<String>,
}

struct Profile {
    name: Option<String>,
    picture_url: Option<String>,
    warning: Option<String>,
}

struct SaveResult {
    name: String,
    relay_confirmed: bool,
}

fn event_profile(pubkey: &str) -> Result<Option<Profile>, String> {
    let events = relay_client::query(
        relay_client::DEFAULT_RELAY_URL,
        serde_json::json!({"kinds": [0], "authors": [pubkey], "limit": 1}),
    )?;
    let Some(event) = events.into_iter().max_by_key(|event| event.created_at) else {
        return Ok(None);
    };
    let metadata: serde_json::Value = serde_json::from_str(&event.content)
        .map_err(|error| format!("invalid profile event: {error}"))?;
    Ok(Some(Profile {
        name: metadata
            .get("display_name")
            .or_else(|| metadata.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        picture_url: metadata
            .get("picture")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        warning: None,
    }))
}

fn cached_profile(pubkey: &str) -> Option<Profile> {
    let path = PathBuf::from("/var/lib/evermore-profiles")
        .join(pubkey)
        .join("profile.json");
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(Profile {
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        picture_url: None,
        warning: None,
    })
}

fn load(identity: &Identity) -> Result<Profile, String> {
    match event_profile(&identity.pubkey_hex) {
        Ok(Some(profile)) => {
            let cached = cached_profile(&identity.pubkey_hex);
            Ok(Profile {
                name: profile
                    .name
                    .or_else(|| cached.as_ref().and_then(|item| item.name.clone())),
                picture_url: profile.picture_url,
                warning: None,
            })
        }
        Ok(None) => Ok(cached_profile(&identity.pubkey_hex).unwrap_or(Profile {
            name: Some(identity.name.clone()),
            picture_url: None,
            warning: None,
        })),
        Err(error) => match cached_profile(&identity.pubkey_hex) {
            Some(mut cached) => {
                cached.warning = Some(format!(
                    "Relay unavailable; showing the local profile cache. {error}"
                ));
                Ok(cached)
            }
            None => Err(format!(
                "Couldn't read the profile from the local relay or cache: {error}"
            )),
        },
    }
}

fn start_load(state: &mut State, identity: &Identity) {
    let identity = identity.clone();
    let name = identity.name.clone();
    let (tx, rx) = mpsc::channel();
    state.loading = Some((name, rx));
    std::thread::spawn(move || {
        let _ = tx.send(load(&identity));
    });
}

fn start_save(
    state: &mut State,
    identity: String,
    name: String,
    pubkey: String,
    picture_url: Option<String>,
) {
    if state.saving.is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    state.saving = Some(rx);
    std::thread::spawn(move || {
        let mut args = vec![
            "set-profile".to_owned(),
            identity,
            "--display-name".to_owned(),
            name.clone(),
        ];
        if let Some(picture_url) = picture_url {
            args.extend(["--picture-url".to_owned(), picture_url]);
        }
        let output = Command::new("evermore-signer")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("Couldn't run evermore-signer: {error}"))
            .and_then(|output| {
                if output.status.success() {
                    Ok(())
                } else {
                    let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    Err(if error.is_empty() {
                        "The signer couldn't save this profile.".into()
                    } else {
                        error
                    })
                }
            });
        let result = output.map(|()| SaveResult {
            relay_confirmed: event_profile(&pubkey)
                .ok()
                .flatten()
                .and_then(|profile| profile.name)
                .is_some_and(|profile_name| profile_name == name),
            name,
        });
        let _ = tx.send(result);
    });
}

pub fn ui(ui: &mut egui::Ui, identities: &[Identity], state: &mut State) {
    let Some(first) = identities.first() else {
        return;
    };
    if !identities
        .iter()
        .any(|identity| identity.name == state.selected)
    {
        state.selected.clone_from(&first.name);
    }
    let previous_selection = state.selected.clone();
    ui.add_enabled_ui(state.saving.is_none(), |ui| {
        egui::ComboBox::from_id_salt("profile-identity")
            .selected_text(&state.selected)
            .show_ui(ui, |ui| {
                for identity in identities {
                    ui.selectable_value(&mut state.selected, identity.name.clone(), &identity.name);
                }
            });
    });
    if state.selected != previous_selection {
        state.loaded_for.clear();
        state.display_name.clear();
        state.original_name.clear();
        state.picture_url = None;
        state.error = None;
        state.status = None;
        state.loading = None;
    }
    let Some(identity) = identities
        .iter()
        .find(|identity| identity.name == state.selected)
    else {
        return;
    };

    let loading_this_identity = state
        .loading
        .as_ref()
        .is_some_and(|(name, _)| name == &identity.name);
    if state.loaded_for != identity.name && !loading_this_identity {
        start_load(state, identity);
    }
    let loaded = state
        .loading
        .as_ref()
        .map(|(name, rx)| (name.clone(), rx.try_recv()));
    if let Some((name, result)) = loaded {
        match result {
            Ok(result) => {
                state.loading = None;
                if name == identity.name {
                    match result {
                        Ok(profile) => {
                            state.display_name =
                                profile.name.unwrap_or_else(|| identity.name.clone());
                            state.original_name.clone_from(&state.display_name);
                            state.picture_url = profile.picture_url;
                            state.error = profile.warning;
                            state.loaded_for = name;
                        }
                        Err(error) => {
                            state.error = Some(error);
                            state.loaded_for = name;
                        }
                    }
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                state.loading = None;
                state.error = Some("Profile lookup stopped unexpectedly.".into());
                state.loaded_for = name;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    let save_result = state.saving.as_ref().map(Receiver::try_recv);
    if let Some(save_result) = save_result {
        match save_result {
            Ok(Ok(result)) => {
                state.display_name = result.name;
                state.original_name.clone_from(&state.display_name);
                state.status = Some(if result.relay_confirmed {
                    "Profile saved; the kind 0 event is visible on the local relay.".into()
                } else {
                    "Profile saved locally; the kind 0 event could not be confirmed on the local relay.".into()
                });
                state.error = None;
                state.saving = None;
            }
            Ok(Err(error)) => {
                state.error = Some(error);
                state.status = None;
                state.saving = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                state.error = Some("Profile update stopped unexpectedly.".into());
                state.saving = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    egui::CollapsingHeader::new("Public profile")
        .default_open(true)
        .show(ui, |ui| {
            ui.weak("Your display name is public NIP-01 profile metadata. It is cached locally for the greeter and published as a kind 0 event on the local relay.");
            if state.loaded_for.is_empty() {
                ui.weak("Loading profile…");
            } else {
                ui.horizontal(|ui| {
                    ui.label("Display name");
                    ui.text_edit_singleline(&mut state.display_name);
                });
                if let Some(picture_url) = &state.picture_url {
                    ui.label(format!("Profile picture URL: {picture_url}"));
                }
                let changed = state.display_name.trim() != state.original_name.as_str();
                if ui
                    .add_enabled(
                        state.saving.is_none()
                            && !state.display_name.trim().is_empty()
                            && state.display_name.trim().len() <= 80,
                        egui::Button::new("Save public profile…"),
                    )
                    .clicked()
                {
                    state.pending_save = Some(state.display_name.trim().to_owned());
                }
                if changed {
                    ui.weak("Profile names are public and can be seen by other relay users.");
                }
            }
            if state.saving.is_some() {
                ui.weak("Saving profile and checking the local relay…");
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
            }
            if let Some(error) = &state.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
                if ui.small_button("Retry profile load").clicked() {
                    state.loaded_for.clear();
                    state.loading = None;
                }
            }
            if let Some(status) = &state.status {
                ui.label(status);
            }
        });

    if state.loading.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(150));
    }

    if let Some(name) = state.pending_save.clone() {
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Confirm public profile update")
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label("This name will be published in a public Nostr kind 0 profile event.");
                ui.label(&name);
                ui.horizontal(|ui| {
                    if ui.button("Publish profile").clicked() {
                        confirm = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if confirm {
            state.pending_save = None;
            let picture_url = state.picture_url.clone();
            start_save(
                state,
                identity.name.clone(),
                name,
                identity.pubkey_hex.clone(),
                picture_url,
            );
        } else if cancel {
            state.pending_save = None;
        }
    }
}
