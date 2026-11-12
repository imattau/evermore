//! The "Evermore AI" drawer: a small always-at-hand window for talking to
//! the local AI directly (`evermore-panel --ai`). Kept in its own module,
//! separate from the full AI *tab* in the main panel (which is the deeper
//! management surface for context sources, indexing, and operation
//! history). The drawer's job is chat first; giving the AI a file or a
//! snippet of text is a lightweight add-on, not the main event.

use eframe::egui;
use egui::AdaptiveLayout;
use evermore_common::ai::Role;

use crate::{
    AiOperationProposal, ContextGrant, ContextScope, PanelApp, RefreshPolicy, ai_context_client,
    ai_handoff, ai_infer_client, parse_ai_operation_proposal, validate_ai_operation_proposal,
};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(110, 168, 254);
/// From the "Evermore Desktop — UI improvements" Figma file's "AI drawer"
/// page -- rendered in the app's dark Evermore Graphite palette rather
/// than that page's light cream/sage mockup (the user's call: everything
/// else in the app is the dark ink/blue theme, and a drawer that looks
/// like a different product isn't the goal). `ACCENT_CHIP` is the same
/// accent-tinted highlight the wizard and the Nostr drawer already use for
/// "this is attached/selected" surfaces; the mockup's equivalent soft-green
/// tint maps to it.
const ACCENT_CHIP: egui::Color32 = egui::Color32::from_rgb(38, 70, 120); // #264678
const ACCENT_INK: egui::Color32 = egui::Color32::from_rgb(7, 9, 12); // #07090C

/// The drawer's one filled accent call-to-action (the operation-approval
/// "Submit for owner approval" button) -- everything else stays the
/// neutral bordered style `build_visuals` now gives every button.
fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).strong().color(ACCENT_INK))
            .fill(ACCENT)
            .corner_radius(8.0),
    )
    .clicked()
}

/// The small uppercase muted caption the design uses above grouped
/// content ("SUGGESTED OPERATION…", "CHATS", "INDEXED FILES"…).
fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(10.0)
            .strong()
            .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
    );
}

impl PanelApp {
    pub(crate) fn ai_drawer_ui(&mut self, ui: &mut egui::Ui) {
        if !self.ai_sessions_loaded {
            self.load_ai_sessions();
        }
        if !self.ai_index_loaded {
            self.refresh_ai_index();
        }
        if let Some(path) = self.pending_ai_file.take() {
            self.attach_file_for_ai(&path);
        }
        if !self.ai_models_loaded {
            self.refresh_ai_models();
        }
        if !self.ai_context_loaded {
            self.refresh_ai_context();
        }
        if !self.operations_loaded {
            self.refresh_operations();
        }
        if !self.identities_loaded {
            self.refresh_identities();
        }
        // A note attached in the separate feed drawer arrives through this
        // runtime handoff; poll it while the drawer is idle so the grant
        // shows up without waiting for another input event.
        if let Some(note) = ai_handoff::take_note() {
            self.attach_note_for_ai(&note);
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));

        let dragging_file = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
        let dropped_files = ui.ctx().input(|input| input.raw.dropped_files.clone());
        for dropped in dropped_files {
            let path = dropped.path();
            if path.as_os_str().is_empty() {
                continue;
            }
            self.attach_file_for_ai(&path.to_string_lossy());
        }

        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                self.ai_drawer_header(ui);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.weak(if self.ai_model_selected.is_empty() {
                    "LOCAL \u{b7} no model selected".to_owned()
                } else {
                    format!("LOCAL \u{b7} {}", self.ai_model_selected)
                });
                ui.add_space(6.0);

                if self.ai_chat_messages.is_empty() {
                    ui.add_space(24.0);
                    ui.label(
                        egui::RichText::new("What would you like to work on?")
                            .size(18.0)
                            .strong(),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(
                            "Your chats stay on this device. Ask a question or attach context to get started.",
                        )
                        .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
                    );
                    ui.add_space(24.0);
                }
                // Measured *after* the empty-state block above (when shown)
                // has already consumed its space -- computing this first and
                // reusing it regardless left the scroll area over-claiming
                // height on a fresh chat, pushing the composer below the
                // visible window on first open. 160 (not the original 118)
                // covers everything actually drawn below the scroll area:
                // the "Local inference…" hint line, the composer, and the
                // always-visible "Advanced context settings" header --
                // confirmed on the dev VM that 118 still clipped the
                // composer's button row against the window's bottom edge.
                let footer_reserve = 160.0;
                let chat_height = (ui.available_height() - footer_reserve).max(140.0);
                egui::ScrollArea::vertical()
                    .id_salt("ai-drawer-chat")
                    .max_height(chat_height)
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for message in self.ai_chat_messages.clone() {
                            let display_content = message
                                .content
                                .lines()
                                .filter(|line| !line.trim_start().starts_with("EVERMORE_PROPOSAL:"))
                                .collect::<Vec<_>>()
                                .join("\n");
                            let loading_placeholder =
                                message.content.is_empty() && self.ai_chat_loading;
                            chat_bubble(ui, message.role, &display_content, loading_placeholder);
                            ui.add_space(6.0);
                            if message.role == Role::Assistant && !self.ai_chat_loading {
                                match parse_ai_operation_proposal(&message.content) {
                                    Ok(Some(proposal)) => self.ai_operation_proposal_ui(ui, proposal),
                                    Ok(None) => {}
                                    Err(error) => {
                                        ui.vertical(|ui| {
                                            ui.strong("Operation proposal");
                                            ui.colored_label(ui.visuals().error_fg_color, error);
                                        });
                                    }
                                }
                            }
                        }
                    });

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Local inference \u{b7} model selection and chat history in \u{22ef}")
                        .size(11.0)
                        .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
                );
                ui.add_space(4.0);
                self.ai_attachment_chips(ui);
                self.ai_input_row(ui, dragging_file);

                if let Some(status) = &self.ai_index_status {
                    ui.weak(status);
                }
                if let Some(error) = &self.ai_context_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if let Some(error) = &self.ai_chat_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }

                if !self.ai_chat_loading && self.ai_chat_input.trim().is_empty() {
                    let suggestions = self.context_prompt_suggestions();
                    if !suggestions.is_empty() {
                        ui.add_space(6.0);
                        egui::Frame::new()
                            .fill(ACCENT_CHIP)
                            .corner_radius(9)
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    egui::RichText::new("TRY WITH SELECTED CONTEXT")
                                        .size(10.0)
                                        .strong()
                                        .color(ACCENT),
                                );
                                ui.add_space(4.0);
                                ui.horizontal_wrapped(|ui| {
                                    for suggestion in suggestions {
                                        if ui.small_button(&suggestion).clicked() {
                                            self.ai_chat_input = suggestion;
                                        }
                                    }
                                });
                            });
                    }
                }

                self.ai_advanced_context_ui(ui);

                if !self.pending_operations.is_empty() {
                    ui.add_space(6.0);
                    ui.collapsing(
                        format!("Pending operation cards ({})", self.pending_operations.len()),
                        |ui| {
                            for pending in self.pending_operations.clone() {
                                ui.vertical(|ui| {
                                    ui.strong(&pending.operation);
                                    ui.weak(format!(
                                        "Request {} · expires {}",
                                        pending.operation_id, pending.expires_at
                                    ));
                                    ui.monospace(
                                        serde_json::to_string_pretty(&pending.arguments)
                                            .unwrap_or_default(),
                                    );
                                    if pending.status_source.is_some() {
                                        ui.weak(
                                            "Offline snapshot · decisions are disabled until the operation service returns.",
                                        );
                                    } else if ui.button("Open signed owner review").clicked()
                                        && let Err(error) = std::process::Command::new("evermore-panel")
                                            .args(["--approval", &pending.operation_id])
                                            .spawn()
                                    {
                                        self.ai_chat_error =
                                            Some(format!("could not open owner review: {error}"));
                                    }
                                });
                                ui.add_space(8.0);
                            }
                        },
                    );
                }
            });
    }

    /// Slim title row: session name plus a single "⋯" menu for everything
    /// that isn't the conversation itself (switching/renaming sessions,
    /// picking a model). Keeps the chat the first thing you see.
    fn ai_drawer_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let title = if self.ai_session_name.trim().is_empty() {
                "Untitled chat"
            } else {
                self.ai_session_name.as_str()
            };
            ui.strong(title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("⋯", |ui| {
                    ui.set_min_width(200.0);
                    caption(ui, "CHAT MENU");
                    if ui.button("\u{2795}  New chat").clicked() {
                        self.new_ai_session();
                        ui.close();
                    }
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.ai_session_name)
                                .hint_text("\u{270e}  Rename current chat"),
                        )
                        .changed()
                    {
                        self.save_ai_session();
                    }
                    if !self.ai_sessions.is_empty() {
                        ui.separator();
                        caption(ui, "CHATS");
                        let mut selected = self.ai_session_id.clone();
                        for session in self.ai_sessions.clone() {
                            let current = session.id == self.ai_session_id;
                            let label = format!(
                                "{}  {}",
                                if current { "\u{25cf}" } else { "\u{25cb}" },
                                session.name
                            );
                            if ui
                                .selectable_value(&mut selected, session.id.clone(), label)
                                .clicked()
                            {
                                ui.close();
                            }
                        }
                        if selected != self.ai_session_id {
                            self.select_ai_session(&selected);
                        }
                    }
                    ui.separator();
                    if self.ai_models.is_empty() {
                        ui.weak("Local inference unavailable on this device.");
                    } else {
                        caption(ui, "LOCAL MODEL");
                        let mut changed = false;
                        for model in self.ai_models.clone() {
                            changed |= ui
                                .selectable_value(
                                    &mut self.ai_model_selected,
                                    model.name.clone(),
                                    &model.name,
                                )
                                .changed();
                        }
                        if changed {
                            let _ =
                                ai_infer_client::save_preferences(&ai_infer_client::Preferences {
                                    model: Some(self.ai_model_selected.clone()),
                                    backend: Some(self.ai_backend),
                                });
                        }
                    }
                });
            });
        });
    }

    /// Small pill per attached file, directly above the input box, so
    /// giving the AI a file is visible where you're about to type - not
    /// something you have to go dig for in a collapsed settings section.
    fn ai_attachment_chips(&mut self, ui: &mut egui::Ui) {
        let chip_grants: Vec<_> = self
            .ai_context_grants
            .iter()
            .filter(|grant| grant.source.starts_with("file:") || grant.source.starts_with("note:"))
            .cloned()
            .collect();
        if chip_grants.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for grant in chip_grants {
                let label = if let Some(path) = grant.source.strip_prefix("file:") {
                    std::path::Path::new(path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    let author = grant
                        .snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.get("author_pubkey"))
                        .and_then(|value| value.as_str())
                        .map(|pubkey| pubkey.chars().take(8).collect::<String>())
                        .unwrap_or_default();
                    format!("Note from {author}\u{2026}")
                };
                egui::Frame::new()
                    .fill(ACCENT_CHIP)
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(label).color(ACCENT)).wrap(),
                            );
                            if ui
                                .small_button("\u{00d7}")
                                .on_hover_text("Remove")
                                .clicked()
                            {
                                self.ai_context_grants.retain(|g| g.source != grant.source);
                                self.save_ai_session();
                            }
                        });
                    });
            }
        });
    }

    /// The input row: a text box that visibly accepts a dropped file (the
    /// border lights up with the accent while you're dragging one over the
    /// window) plus an explicit "Attach" menu for picking a file without
    /// drag-and-drop. Notes from the feed drawer arrive through the runtime
    /// handoff instead, because egui cannot drag across native windows.
    fn ai_input_row(&mut self, ui: &mut egui::Ui, dragging_file: bool) {
        let border = if dragging_file {
            egui::Stroke::new(2.0, ACCENT)
        } else {
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
        };
        // The drawer is normally 400 points wide, but Sway can shrink it
        // when the workspace is split. Keep a useful multiline composer
        // without letting it crowd out the attach and send controls.
        let desired_rows = AdaptiveLayout::breakpoint(340.0)
            .show(ui, |_| 3, |_| 2)
            .inner;
        egui::Frame::new()
            .stroke(border)
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.ai_chat_input)
                        .hint_text(if dragging_file {
                            "Drop to attach…"
                        } else {
                            "Ask locally, or drop a file in to give it context"
                        })
                        .desired_rows(desired_rows)
                        .frame(egui::Frame::NONE),
                );
            });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.menu_button("Attach", |ui| {
                ui.set_min_width(220.0);
                ui.strong("Add context");
                ui.add_space(6.0);
                if self.ai_index_scopes.is_empty() {
                    ui.weak("No indexed files yet. Drop one on this window, or add a path below.");
                } else {
                    caption(ui, "INDEXED FILES");
                    for scope in self.ai_index_scopes.clone() {
                        let label = std::path::Path::new(&scope.path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        if ui.button(label).clicked() {
                            self.attach_file_for_ai(&scope.path);
                            ui.close();
                        }
                    }
                    ui.separator();
                }
                caption(ui, "OR ENTER A TEXT FILE PATH");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.ai_index_path)
                            .hint_text("Path to a text file"),
                    );
                    if primary_button(ui, "Attach file", !self.ai_index_path.trim().is_empty()) {
                        let path = self.ai_index_path.clone();
                        self.attach_file_for_ai(&path);
                        self.ai_index_path.clear();
                        ui.close();
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("You can also drop a file onto the drawer.")
                        .size(10.0)
                        .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.ai_chat_loading {
                    if ui.button("Cancel").clicked() {
                        self.cancel_ai_chat();
                    }
                } else {
                    if primary_button(
                        ui,
                        "Send",
                        !self.ai_models.is_empty() && !self.ai_chat_input.trim().is_empty(),
                    ) {
                        self.send_ai_chat();
                    }
                    if ui.small_button("Clear").clicked() {
                        self.ai_chat_messages.clear();
                        self.ai_chat_error = None;
                        self.save_ai_session();
                    }
                }
            });
        });
    }

    /// Everything that isn't basic chat-plus-attach: the per-source
    /// allowlist, scope (one turn vs. pinned), refresh policy, and the raw
    /// context payload preview. Real settings, kept out of the way of the
    /// primary flow instead of expanded by default.
    fn ai_advanced_context_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.collapsing("Advanced context settings", |ui| {
            ui.label("Only sources allowed by ai-contextd can be added. One-turn grants are removed after a request; pinned grants remain until you remove them.");
            let before = serde_json::to_value(&self.ai_context_grants).unwrap_or_default();
            for (source, label) in ai_context_client::SOURCES {
                let allowed = self.ai_context_sources.iter().any(|item| item == source);
                let current = self
                    .ai_context_grants
                    .iter()
                    .find(|grant| grant.source == *source);
                let mut selected = current.is_some();
                let mut scope = current.map(|grant| grant.scope).unwrap_or_default();
                let mut refresh = current.map(|grant| grant.refresh).unwrap_or_default();
                let mut changed = false;
                let mut read_now = false;
                ui.horizontal(|ui| {
                    ui.label(*label);
                    if allowed {
                        changed = ui.checkbox(&mut selected, "Use").changed();
                    } else {
                        ui.weak("Not allowlisted");
                    }
                    if allowed && selected {
                        egui::ComboBox::from_id_salt(("drawer-scope", source))
                            .selected_text(match scope {
                                ContextScope::Turn => "Turn",
                                ContextScope::Pinned => "Pinned",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut scope, ContextScope::Turn, "One turn");
                                ui.selectable_value(&mut scope, ContextScope::Pinned, "Pinned");
                            });
                        egui::ComboBox::from_id_salt(("drawer-refresh", source))
                            .selected_text(match refresh {
                                RefreshPolicy::Snapshot => "Snapshot",
                                RefreshPolicy::EachTurn => "Each turn",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut refresh, RefreshPolicy::Snapshot, "Snapshot");
                                ui.selectable_value(&mut refresh, RefreshPolicy::EachTurn, "Each turn");
                            });
                        read_now = ui.small_button("Read").clicked();
                    }
                });
                if changed {
                    if selected {
                        self.ai_context_grants.push(ContextGrant {
                            source: (*source).into(),
                            scope,
                            refresh,
                            snapshot: None,
                        });
                    } else {
                        self.ai_context_grants.retain(|grant| grant.source != *source);
                    }
                }
                if let Some(grant) = self
                    .ai_context_grants
                    .iter_mut()
                    .find(|grant| grant.source == *source)
                {
                    grant.scope = scope;
                    if grant.refresh != refresh {
                        grant.refresh = refresh;
                        if refresh == RefreshPolicy::EachTurn {
                            grant.snapshot = None;
                        }
                    }
                }
                if read_now {
                    match ai_context_client::get(source) {
                        Ok(data) => {
                            if let Some(grant) = self
                                .ai_context_grants
                                .iter_mut()
                                .find(|grant| grant.source == *source)
                            {
                                grant.snapshot = Some(data);
                            }
                            self.ai_context_error = None;
                        }
                        Err(error) => self.ai_context_error = Some(error),
                    }
                }
            }
            if before != serde_json::to_value(&self.ai_context_grants).unwrap_or_default() {
                self.save_ai_session();
            }
            ui.add_space(4.0);
            ui.label(format!(
                "Last request: about {} context tokens and {} input tokens · model window {} · 512 token response reserve",
                self.ai_context_token_estimate,
                self.ai_input_token_estimate,
                self.ai_models
                    .iter()
                    .find(|model| model.name == self.ai_model_selected)
                    .and_then(|model| model.context_size)
                    .unwrap_or(4096)
            ));
            if self.ai_context_preview.is_empty() {
                ui.weak("No context payload has been sent yet.");
            } else {
                let redactions = self.ai_context_preview.matches("[REDACTED]").count()
                    + usize::from(self.ai_context_preview.contains("[REDACTED PRIVATE KEY BLOCK]"));
                ui.label(format!("Redaction markers: {redactions} · truncation: none"));
                ui.add(
                    egui::TextEdit::multiline(&mut self.ai_context_preview)
                        .desired_rows(6)
                        .code_editor(),
                );
            }
        });
    }

    fn ai_operation_proposal_ui(&mut self, ui: &mut egui::Ui, proposal: AiOperationProposal) {
        let validation = validate_ai_operation_proposal(&proposal, &self.operations);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let eligible_actors = match &validation {
            Ok((definition, _)) => self
                .identities
                .iter()
                .filter(|identity| {
                    self.configured_owner_pubkey.as_deref() != Some(identity.pubkey_hex.as_str())
                        && self.operation_grants.iter().any(|grant| {
                            grant.agent_pubkey == identity.pubkey_hex
                                && grant.grant_published
                                && !grant.revoked
                                && now < grant.expires_at
                                && grant
                                    .allowed_operations
                                    .iter()
                                    .any(|operation| operation == &definition.name)
                        })
                })
                .map(|identity| identity.name.clone())
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        if !eligible_actors
            .iter()
            .any(|identity| identity == &self.proposal_identity)
        {
            self.proposal_identity = eligible_actors.first().cloned().unwrap_or_default();
        }
        egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .stroke(egui::Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                        caption(ui, "SUGGESTED OPERATION \u{b7} REVIEW BEFORE SUBMITTING");
                        ui.add_space(4.0);
                        match &validation {
                            Err(error) => {
                                ui.colored_label(ui.visuals().error_fg_color, error);
                            }
                            Ok((definition, arguments)) => {
                                ui.strong(&definition.name);
                                ui.weak(&definition.description);
                                ui.add_space(4.0);
                                egui::Frame::new()
                                    .fill(ui.visuals().extreme_bg_color)
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        ui.visuals().widgets.inactive.bg_stroke.color,
                                    ))
                                    .corner_radius(7)
                                    .inner_margin(egui::Margin::symmetric(8, 6))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.monospace(
                                            serde_json::to_string_pretty(arguments)
                                                .unwrap_or_else(|_| "{}".into()),
                                        );
                                    });
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new("Requesting identity")
                                        .size(11.0)
                                        .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
                                );
                                egui::ComboBox::from_id_salt(("ai-proposal-actor", &definition.name))
                                    .selected_text(if self.proposal_identity.is_empty() {
                                        "No eligible requesting identity"
                                    } else {
                                        &self.proposal_identity
                                    })
                                    .show_ui(ui, |ui| {
                                        for identity in &eligible_actors {
                                            ui.selectable_value(
                                                &mut self.proposal_identity,
                                                identity.clone(),
                                                identity,
                                            );
                                        }
                                    });
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("Configured owner identity")
                                        .size(11.0)
                                        .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
                                );
                                egui::ComboBox::from_id_salt(("ai-proposal-owner", &definition.name))
                                    .selected_text(if self.approval_identity.is_empty() {
                                        "Configured owner unavailable"
                                    } else {
                                        &self.approval_identity
                                    })
                                    .show_ui(ui, |ui| {
                                        for identity in self.identities.iter().filter(|identity| {
                                            self.configured_owner_pubkey.as_deref().is_some_and(
                                                |owner| identity.pubkey_hex == owner,
                                            )
                                        }) {
                                            ui.selectable_value(
                                                &mut self.approval_identity,
                                                identity.name.clone(),
                                                &identity.name,
                                            );
                                        }
                                    });
                                let actor = self
                                    .identities
                                    .iter()
                                    .find(|identity| identity.name == self.proposal_identity);
                                let owner = self
                                    .identities
                                    .iter()
                                    .find(|identity| identity.name == self.approval_identity);
                                let configured_owner_matches = owner.is_some_and(|identity| {
                                    self.configured_owner_pubkey
                                        .as_deref()
                                        .is_some_and(|owner| identity.pubkey_hex == owner)
                                });
                                let actor_has_active_grant = actor.is_some_and(|actor| {
                                    self.operation_grants.iter().any(|grant| {
                                        grant.agent_pubkey == actor.pubkey_hex
                                            && grant.grant_published
                                            && !grant.revoked
                                            && now < grant.expires_at
                                            && grant
                                                .allowed_operations
                                                .iter()
                                                .any(|operation| operation == &definition.name)
                                    })
                                });
                                let distinct = actor.zip(owner).is_some_and(|(actor, owner)| {
                                    actor.pubkey_hex != owner.pubkey_hex
                                });
                                let identities_available =
                                    actor_has_active_grant && configured_owner_matches;
                                if !identities_available {
                                    if let Some(error) = &self.operation_grants_error {
                                        ui.weak(format!("Could not load operation grants: {error}"));
                                    } else if let Some(error) = &self.identity_error {
                                        ui.weak(format!("Could not load signing identities: {error}"));
                                    } else if self.identities.is_empty() {
                                        ui.weak("No signing identities are loaded in this session.");
                                    } else if !configured_owner_matches {
                                        ui.weak(
                                            "The configured owner identity is not loaded in this session.",
                                        );
                                    } else if eligible_actors.is_empty() {
                                        ui.weak(
                                            "No loaded requesting identity has an active, published grant for this operation.",
                                        );
                                    }
                                } else if !distinct {
                                    ui.colored_label(
                                        ui.visuals().error_fg_color,
                                        "Choose a separate owner identity; an agent cannot approve its own request.",
                                    );
                                } else {
                                    ui.weak(
                                        "Submitting creates a signed pending request. It does not run the operation.",
                                    );
                                }
                                let owner_pubkey = owner.map(|identity| identity.pubkey_hex.clone());
                                if primary_button(
                                    ui,
                                    "Submit for owner approval",
                                    identities_available
                                        && distinct
                                        && owner_pubkey.is_some()
                                        && self.proposal_status.is_none(),
                                ) {
                                    self.proposal_operation = proposal.operation.clone();
                                    self.proposal_arguments = arguments.to_string();
                                    self.submit_proposal(owner_pubkey.as_deref().unwrap_or_default());
                                }
                                if let Some(status) = &self.proposal_status {
                                    ui.weak(status);
                                }
                                if let Some(error) = &self.proposal_error {
                                    ui.colored_label(ui.visuals().error_fg_color, error);
                                }
                            }
                        }
                    });
            });
    }
}

/// Renders one chat message as a bubble: the user's turns hug the right
/// edge in an accent-tinted fill, the AI's hug the left in the card fill,
/// and system notices sit centered as plain weak text (no bubble - they're
/// not part of the conversation).
fn chat_bubble(ui: &mut egui::Ui, role: Role, content: &str, loading_placeholder: bool) {
    let text = if loading_placeholder {
        "Thinking…"
    } else {
        content
    };
    match role {
        Role::User => {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                bubble_frame(
                    ui,
                    ACCENT.gamma_multiply(0.22),
                    ui.visuals().strong_text_color(),
                    text,
                );
            });
        }
        Role::Assistant => {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                bubble_frame(
                    ui,
                    ui.visuals().faint_bg_color,
                    ui.visuals().text_color(),
                    text,
                );
            });
        }
        Role::System => {
            ui.vertical_centered(|ui| {
                ui.weak(text);
            });
        }
    }
}

fn bubble_frame(ui: &mut egui::Ui, fill: egui::Color32, text_color: egui::Color32, content: &str) {
    let max_width = (ui.available_width() * 0.82).max(140.0);
    egui::Frame::new()
        .fill(fill)
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_max_width(max_width);
            ui.add(egui::Label::new(egui::RichText::new(content).color(text_color)).wrap());
        });
}
