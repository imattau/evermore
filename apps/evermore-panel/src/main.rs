mod ai_context_client;
mod ai_index_client;
mod ai_infer_client;
mod notifications_client;
mod npack_client;
mod operations_client;
mod settings_client;
mod signer_client;

use eframe::egui;
use evermore_common::ai::{Backend, Message, Role};
use evermore_common::notifications::{NotificationRecord, Source};
use evermore_common::operations::Definition;
use signer_client::Identity;

fn main() -> eframe::Result {
    let arguments: Vec<String> = std::env::args().collect();
    let command_palette = arguments.iter().any(|arg| arg == "--command-palette");
    let open_ai_tab = arguments.iter().any(|arg| arg == "--ai");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(if command_palette {
                [720.0, 560.0]
            } else {
                [420.0, 560.0]
            })
            .with_title(if command_palette {
                "Evermore command palette"
            } else {
                "Evermore"
            }),
        ..Default::default()
    };

    eframe::run_native(
        "Evermore",
        options,
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            let mut app = PanelApp::default();
            app.command_palette = command_palette;
            if open_ai_tab {
                app.tab = Tab::Ai;
            }
            if command_palette {
                app.tab = Tab::Ai;
                app.proposal_arguments = "{}".into();
            }
            Ok(Box::new(app))
        }),
    )
}

/// Placeholder tabs for the daemons this panel will eventually talk to
/// (signer, settings, notifications, packages). Phase 1 wires the first
/// one up for real.
#[derive(Default, PartialEq)]
enum Tab {
    #[default]
    Identity,
    Settings,
    Notifications,
    Ai,
    Packages,
}

#[derive(Default)]
struct PanelApp {
    tab: Tab,
    identities: Vec<Identity>,
    identity_error: Option<String>,
    identities_loaded: bool,
    theme_domain: serde_json::Map<String, serde_json::Value>,
    settings_error: Option<String>,
    settings_loaded: bool,
    notifications: Vec<NotificationRecord>,
    notifications_error: Option<String>,
    notifications_loaded: bool,
    operations: Vec<Definition>,
    operations_error: Option<String>,
    operations_loaded: bool,
    pending_operations: Vec<operations_client::PendingOperation>,
    operation_history: Vec<operations_client::OperationHistoryEntry>,
    approval_identity: String,
    approval_error: Option<String>,
    approval_status: Option<String>,
    proposal_identity: String,
    proposal_operation: String,
    proposal_arguments: String,
    command_palette: bool,
    palette_query: String,
    proposal_status: Option<String>,
    proposal_error: Option<String>,
    npack_identity: String,
    npack_blossom_url: String,
    npack_trust_input: String,
    npack_installed: Vec<npack_client::InstalledPackage>,
    npack_trusted: Vec<String>,
    npack_available: Vec<npack_client::AvailablePackage>,
    npack_error: Option<String>,
    npack_loaded: bool,
    ai_index_scopes: Vec<evermore_common::ai_index::Scope>,
    ai_index_loaded: bool,
    ai_index_path: String,
    ai_index_query: String,
    ai_index_results: Vec<evermore_common::ai_index::SearchResult>,
    ai_index_error: Option<String>,
    ai_index_status: Option<String>,
    ai_nostr_files: Vec<String>,
    ai_nostr_discovered: bool,
    ai_context_sources: Vec<String>,
    ai_context_loaded: bool,
    ai_context_data: Option<(String, serde_json::Value)>,
    ai_context_error: Option<String>,
    ai_models: Vec<evermore_common::ai::Model>,
    ai_models_loaded: bool,
    ai_preferences_loaded: bool,
    ai_model_selected: String,
    ai_backend: Backend,
    ai_chat_input: String,
    ai_chat_messages: Vec<Message>,
    ai_chat_receiver: Option<std::sync::mpsc::Receiver<ai_infer_client::ChatUpdate>>,
    ai_chat_loading: bool,
    ai_chat_error: Option<String>,
}

impl PanelApp {
    fn refresh_ai_models(&mut self) {
        self.ai_models_loaded = true;
        if !self.ai_preferences_loaded {
            let preferences = ai_infer_client::load_preferences();
            self.ai_model_selected = preferences.model.unwrap_or_default();
            self.ai_backend = preferences.backend.unwrap_or_default();
            self.ai_preferences_loaded = true;
        }
        let stored_model = self.ai_model_selected.clone();
        match ai_infer_client::list_models() {
            Ok(models) => {
                self.ai_models = models;
                if !self
                    .ai_models
                    .iter()
                    .any(|model| model.name == self.ai_model_selected)
                {
                    self.ai_model_selected = self
                        .ai_models
                        .first()
                        .map(|model| model.name.clone())
                        .unwrap_or_default();
                }
                if stored_model != self.ai_model_selected && !self.ai_model_selected.is_empty() {
                    let _ = ai_infer_client::save_preferences(&ai_infer_client::Preferences {
                        model: Some(self.ai_model_selected.clone()),
                        backend: Some(self.ai_backend),
                    });
                }
                self.ai_chat_error = None;
            }
            Err(error) => self.ai_chat_error = Some(error),
        }
    }

    fn poll_ai_chat(&mut self) {
        let mut finished = false;
        if let Some(receiver) = &self.ai_chat_receiver {
            loop {
                match receiver.try_recv() {
                    Ok(ai_infer_client::ChatUpdate::Token(token)) => {
                        if let Some(message) = self.ai_chat_messages.last_mut()
                            && message.role == Role::Assistant
                        {
                            message.content.push_str(&token);
                        }
                    }
                    Ok(ai_infer_client::ChatUpdate::Complete) => finished = true,
                    Ok(ai_infer_client::ChatUpdate::Error(error)) => {
                        self.ai_chat_error = Some(error);
                        finished = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        if self.ai_chat_loading {
                            self.ai_chat_error = Some("inference stream disconnected".into());
                        }
                        finished = true;
                    }
                }
            }
        }
        if finished {
            self.ai_chat_loading = false;
            self.ai_chat_receiver = None;
        }
    }

    fn send_ai_chat(&mut self) {
        let prompt = self.ai_chat_input.trim().to_string();
        if prompt.is_empty() || self.ai_model_selected.is_empty() || self.ai_chat_loading {
            return;
        }
        self.ai_chat_messages.push(Message {
            role: Role::User,
            content: evermore_common::ai::redact_text(&prompt),
        });
        let messages_to_send = self
            .ai_chat_messages
            .iter()
            .map(|message| Message {
                role: message.role,
                content: evermore_common::ai::redact_text(&message.content),
            })
            .collect();
        self.ai_chat_messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
        });
        self.ai_chat_input.clear();
        self.ai_chat_error = None;
        self.ai_chat_loading = true;
        self.ai_chat_receiver = Some(ai_infer_client::start_chat(
            self.ai_model_selected.clone(),
            self.ai_backend,
            messages_to_send,
        ));
    }

    fn refresh_identities(&mut self) {
        self.identities_loaded = true;
        match signer_client::fetch_identities() {
            Ok(identities) => {
                if self.approval_identity.is_empty() {
                    self.approval_identity = identities
                        .first()
                        .map(|identity| identity.name.clone())
                        .unwrap_or_default();
                }
                if self.proposal_identity.is_empty() {
                    self.proposal_identity = identities
                        .first()
                        .map(|identity| identity.name.clone())
                        .unwrap_or_default();
                }
                self.identities = identities;
                self.identity_error = None;
            }
            Err(e) => {
                self.identities.clear();
                self.identity_error = Some(e);
            }
        }
    }

    fn refresh_theme(&mut self) {
        self.settings_loaded = true;
        match settings_client::get_domain("theme") {
            Ok(map) => {
                self.theme_domain = map;
                self.settings_error = None;
            }
            Err(e) => {
                self.theme_domain.clear();
                self.settings_error = Some(e);
            }
        }
    }

    fn set_theme_mode(&mut self, mode: &str) {
        match settings_client::set("theme", "mode", serde_json::Value::String(mode.to_string())) {
            Ok(()) => self.refresh_theme(),
            Err(e) => self.settings_error = Some(e),
        }
    }

    fn refresh_notifications(&mut self) {
        self.notifications_loaded = true;
        match notifications_client::list_recent() {
            Ok(records) => {
                self.notifications = records;
                self.notifications_error = None;
            }
            Err(e) => {
                self.notifications.clear();
                self.notifications_error = Some(e);
            }
        }
    }

    fn refresh_operations(&mut self) {
        self.operations_loaded = true;
        match operations_client::fetch_catalog() {
            Ok(operations) => {
                self.operations = operations;
                self.operations_error = None;
            }
            Err(error) => {
                self.operations = evermore_common::operations::catalog();
                self.operations_error = Some(format!(
                    "Executor unavailable; showing the local catalogue and journal snapshot: {error}"
                ));
            }
        }
        match operations_client::pending_operations() {
            Ok(operations) => {
                self.pending_operations = operations;
                self.approval_error = None;
            }
            Err(error) => self.approval_error = Some(error),
        }
        match operations_client::history(20) {
            Ok(history) => self.operation_history = history,
            Err(error) => self.approval_error = Some(error),
        }
    }

    fn refresh_npack(&mut self) {
        self.npack_loaded = true;
        self.npack_error = None;

        match npack_client::installed() {
            Ok(installed) => self.npack_installed = installed,
            Err(e) => self.npack_error = Some(e),
        }

        if self.npack_identity.trim().is_empty() {
            self.npack_trusted.clear();
            self.npack_available.clear();
            return;
        }
        match npack_client::trusted(&self.npack_identity) {
            Ok(trusted) => self.npack_trusted = trusted,
            Err(e) => self.npack_error = Some(e),
        }
        match npack_client::available(&self.npack_identity) {
            Ok(available) => self.npack_available = available,
            Err(e) => self.npack_error = Some(e),
        }
    }

    fn refresh_ai_index(&mut self) {
        self.ai_index_loaded = true;
        match ai_index_client::list_scopes() {
            Ok(scopes) => {
                self.ai_index_scopes = scopes;
                self.ai_index_error = None;
            }
            Err(error) => {
                self.ai_index_scopes.clear();
                self.ai_index_error = Some(error);
            }
        }
    }

    fn refresh_ai_context(&mut self) {
        self.ai_context_loaded = true;
        match ai_context_client::list_sources() {
            Ok(sources) => {
                self.ai_context_sources = sources;
                self.ai_context_error = None;
            }
            Err(error) => {
                self.ai_context_sources.clear();
                self.ai_context_error = Some(error);
            }
        }
    }

    fn submit_proposal(&mut self, owner_pubkey: &str) {
        let result = serde_json::from_str::<serde_json::Value>(&self.proposal_arguments)
            .map_err(|error| format!("invalid JSON arguments: {error}"))
            .and_then(|arguments| {
                operations_client::submit(
                    &self.proposal_identity,
                    owner_pubkey,
                    &self.proposal_operation,
                    arguments,
                )
            });
        match result {
            Ok(operation_id) => {
                self.proposal_status = Some(format!("Submitted operation {operation_id}"));
                self.proposal_error = None;
                self.refresh_operations();
            }
            Err(error) => self.proposal_error = Some(error),
        }
    }

    fn command_palette_ui(&mut self, ui: &mut egui::Ui) {
        if !self.operations_loaded {
            self.refresh_operations();
        }
        if !self.identities_loaded {
            self.refresh_identities();
        }
        ui.heading("Evermore command palette");
        ui.label("Search a catalogued operation. Every write still needs its exact grant and owner approval.");
        ui.add_space(8.0);
        ui.label("Find an operation");
        ui.add(
            egui::TextEdit::singleline(&mut self.palette_query)
                .hint_text("Search settings, packages, notifications, or Sway"),
        );
        let writable: Vec<_> = self
            .operations
            .iter()
            .filter(|definition| {
                definition.access == evermore_common::operations::Access::Write
                    && definition.approval == evermore_common::operations::Approval::Required
                    && (self.palette_query.trim().is_empty()
                        || definition
                            .name
                            .to_ascii_lowercase()
                            .contains(&self.palette_query.to_ascii_lowercase())
                        || definition
                            .description
                            .to_ascii_lowercase()
                            .contains(&self.palette_query.to_ascii_lowercase()))
            })
            .collect();
        if !writable
            .iter()
            .any(|definition| definition.name == self.proposal_operation)
            && let Some(first) = writable.first()
        {
            self.proposal_operation = first.name.clone();
        }
        egui::ComboBox::from_label("Operation")
            .selected_text(if self.proposal_operation.is_empty() {
                "No matching write operations"
            } else {
                &self.proposal_operation
            })
            .show_ui(ui, |ui| {
                for definition in &writable {
                    ui.selectable_value(
                        &mut self.proposal_operation,
                        definition.name.clone(),
                        &definition.name,
                    );
                }
            });
        if let Some(definition) = self
            .operations
            .iter()
            .find(|definition| definition.name == self.proposal_operation)
        {
            ui.weak(&definition.description);
            ui.collapsing("Input schema", |ui| {
                ui.monospace(
                    serde_json::to_string_pretty(&definition.input_schema)
                        .unwrap_or_else(|_| "{}".into()),
                );
            });
        }
        ui.add_space(6.0);
        egui::ComboBox::from_label("Request as")
            .selected_text(if self.proposal_identity.is_empty() {
                "Select an agent identity"
            } else {
                &self.proposal_identity
            })
            .show_ui(ui, |ui| {
                for identity in &self.identities {
                    ui.selectable_value(
                        &mut self.proposal_identity,
                        identity.name.clone(),
                        &identity.name,
                    );
                }
            });
        egui::ComboBox::from_label("Encrypt to owner")
            .selected_text(if self.approval_identity.is_empty() {
                "Select the configured owner"
            } else {
                &self.approval_identity
            })
            .show_ui(ui, |ui| {
                for identity in &self.identities {
                    ui.selectable_value(
                        &mut self.approval_identity,
                        identity.name.clone(),
                        &identity.name,
                    );
                }
            });
        ui.label("Arguments (JSON)");
        ui.add(
            egui::TextEdit::multiline(&mut self.proposal_arguments)
                .desired_rows(5)
                .code_editor(),
        );
        let owner_pubkey = self
            .identities
            .iter()
            .find(|identity| identity.name == self.approval_identity)
            .map(|identity| identity.pubkey_hex.clone());
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.proposal_identity.is_empty()
                        && owner_pubkey.is_some()
                        && !self.proposal_operation.is_empty(),
                    egui::Button::new("Submit for owner approval"),
                )
                .clicked()
            {
                self.submit_proposal(owner_pubkey.as_deref().unwrap_or_default());
            }
            if ui.button("Close").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        if let Some(status) = &self.proposal_status {
            ui.weak(status);
        }
        if let Some(error) = &self.proposal_error {
            ui.colored_label(egui::Color32::from_rgb(230, 120, 110), error);
        }
        if let Some(error) = &self.identity_error {
            ui.colored_label(egui::Color32::from_rgb(230, 120, 110), error);
        }
        if let Some(error) = &self.operations_error {
            ui.colored_label(egui::Color32::from_rgb(230, 170, 110), error);
        }
    }
}

impl eframe::App for PanelApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_ai_chat();
        if self.ai_chat_loading {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(80));
        }
        if self.command_palette {
            self.command_palette_ui(ui);
            return;
        }
        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.selectable_value(&mut self.tab, Tab::Identity, "Identity");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                ui.selectable_value(&mut self.tab, Tab::Notifications, "Notifications");
                ui.selectable_value(&mut self.tab, Tab::Ai, "AI");
                ui.selectable_value(&mut self.tab, Tab::Packages, "Packages");
            });
            ui.add_space(6.0);
        });

        ui.add_space(16.0);
        match self.tab {
            Tab::Identity => {
                if !self.identities_loaded {
                    self.refresh_identities();
                }
                ui.heading("Identity");
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        self.refresh_identities();
                    }
                    ui.label(evermore_common::runtime_socket_path().display().to_string());
                });
                ui.add_space(8.0);

                if let Some(err) = &self.identity_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), err);
                } else if self.identities.is_empty() {
                    ui.label(
                        "No identities loaded. Create one on the host with \
                         `evermore-signer keygen <name>`, then Refresh.",
                    );
                } else {
                    for identity in &self.identities {
                        ui.group(|ui| {
                            ui.label(egui::RichText::new(&identity.name).strong());
                            ui.monospace(&identity.npub);
                            ui.weak(&identity.pubkey_hex);
                        });
                        ui.add_space(6.0);
                    }
                }
            }
            Tab::Settings => {
                if !self.settings_loaded {
                    self.refresh_theme();
                }
                ui.heading("Settings");
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        self.refresh_theme();
                    }
                    ui.label(
                        evermore_common::settings::runtime_socket_path()
                            .display()
                            .to_string(),
                    );
                });
                ui.add_space(8.0);

                if let Some(err) = &self.settings_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), err);
                    ui.add_space(8.0);
                }

                let current_mode = self
                    .theme_domain
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unset)")
                    .to_string();
                ui.label(format!(
                    "evermore:theme.mode = {current_mode} (synced via the control-plane relay, kind:30078)"
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Set dark").clicked() {
                        self.set_theme_mode("dark");
                    }
                    if ui.button("Set light").clicked() {
                        self.set_theme_mode("light");
                    }
                });
            }
            Tab::Notifications => {
                if !self.notifications_loaded {
                    self.refresh_notifications();
                }
                ui.heading("Notifications");
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        self.refresh_notifications();
                    }
                    ui.label(
                        evermore_common::notifications::runtime_socket_path()
                            .display()
                            .to_string(),
                    );
                });
                ui.add_space(8.0);

                if let Some(err) = &self.notifications_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), err);
                } else if self.notifications.is_empty() {
                    ui.label("No notifications yet.");
                } else {
                    for n in &self.notifications {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(source_label(n.source)).strong());
                                ui.weak(&n.title);
                            });
                            ui.label(&n.body);
                        });
                        ui.add_space(6.0);
                    }
                }
            }
            Tab::Ai => {
                if !self.operations_loaded {
                    self.refresh_operations();
                }
                if !self.identities_loaded {
                    self.refresh_identities();
                }
                if !self.ai_index_loaded {
                    self.refresh_ai_index();
                }
                if !self.ai_context_loaded {
                    self.refresh_ai_context();
                }
                if !self.ai_models_loaded {
                    self.refresh_ai_models();
                }
                ui.heading("AI capabilities");
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        self.refresh_operations();
                        self.refresh_ai_models();
                    }
                    ui.label(
                        evermore_common::operations::runtime_socket_path()
                            .display()
                            .to_string(),
                    );
                });
                ui.add_space(8.0);
                ui.heading("Local model");
                ui.label("Prompts run through the local inference service. Secret-shaped values are redacted before submission; this panel has no cloud inference path.");
                if self.ai_models.is_empty() {
                    if self.ai_chat_error.is_some() {
                        ui.weak("The local inference service is unavailable. Start evermore-inferd, then refresh this view.");
                    } else {
                        ui.weak("No local models are installed yet.");
                        ui.label("In Packages, install a trusted signed package that declares a GGUF model. Then propose ai.model.install_signed_npack with that package ID and approve it here. Return and refresh this list when import finishes.");
                    }
                } else {
                    let mut preferences_changed = false;
                    egui::ComboBox::from_label("Model")
                        .selected_text(if self.ai_model_selected.is_empty() {
                            "Choose a local model"
                        } else {
                            &self.ai_model_selected
                        })
                        .show_ui(ui, |ui| {
                            for model in &self.ai_models {
                                preferences_changed |= ui
                                    .selectable_value(
                                        &mut self.ai_model_selected,
                                        model.name.clone(),
                                        &model.name,
                                    )
                                    .changed();
                            }
                        });
                    egui::ComboBox::from_label("Backend")
                        .selected_text(match self.ai_backend {
                            Backend::Auto => "Auto",
                            Backend::Cpu => "CPU",
                            Backend::Gpu => "GPU",
                        })
                        .show_ui(ui, |ui| {
                            for (backend, label) in [
                                (Backend::Auto, "Auto"),
                                (Backend::Cpu, "CPU"),
                                (Backend::Gpu, "GPU"),
                            ] {
                                preferences_changed |= ui
                                    .selectable_value(&mut self.ai_backend, backend, label)
                                    .changed();
                            }
                        });
                    if preferences_changed
                        && let Err(error) =
                            ai_infer_client::save_preferences(&ai_infer_client::Preferences {
                                model: Some(self.ai_model_selected.clone()),
                                backend: Some(self.ai_backend),
                            })
                    {
                        self.ai_chat_error =
                            Some(format!("could not save model selection: {error}"));
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("local-chat-history")
                        .max_height(190.0)
                        .show(ui, |ui| {
                            for message in &self.ai_chat_messages {
                                ui.group(|ui| {
                                    let role = match message.role {
                                        Role::User => "You",
                                        Role::Assistant => "Local model",
                                        Role::System => "System",
                                    };
                                    ui.strong(role);
                                    ui.label(
                                        if message.content.is_empty() && self.ai_chat_loading {
                                            "Thinking…"
                                        } else {
                                            &message.content
                                        },
                                    );
                                });
                            }
                        });
                    ui.add(
                        egui::TextEdit::multiline(&mut self.ai_chat_input)
                            .hint_text("Ask your local model")
                            .desired_rows(2),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self.ai_chat_loading
                                    && !self.ai_model_selected.is_empty()
                                    && !self.ai_chat_input.trim().is_empty(),
                                egui::Button::new(if self.ai_chat_loading {
                                    "Generating…"
                                } else {
                                    "Send locally"
                                }),
                            )
                            .clicked()
                        {
                            self.send_ai_chat();
                        }
                        if ui.button("Clear chat").clicked() && !self.ai_chat_loading {
                            self.ai_chat_messages.clear();
                            self.ai_chat_error = None;
                        }
                    });
                }
                if let Some(error) = &self.ai_chat_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), error);
                }

                ui.separator();
                ui.label(
                    "Shared operation catalogue. Write requests are signed, reviewed here, and executed only by the operation daemon after approval.",
                );
                ui.add_space(8.0);

                ui.separator();
                ui.heading("Propose an operation");
                ui.label("Choose the signing identity. The request is encrypted to the configured owner and must pass its grant and approval checks.");
                egui::ComboBox::from_label("Request as")
                    .selected_text(if self.proposal_identity.is_empty() {
                        "Select an identity"
                    } else {
                        &self.proposal_identity
                    })
                    .show_ui(ui, |ui| {
                        for identity in &self.identities {
                            ui.selectable_value(
                                &mut self.proposal_identity,
                                identity.name.clone(),
                                &identity.name,
                            );
                        }
                    });
                let writable: Vec<_> = self
                    .operations
                    .iter()
                    .filter(|definition| {
                        definition.access == evermore_common::operations::Access::Write
                            && definition.approval
                                == evermore_common::operations::Approval::Required
                    })
                    .map(|definition| definition.name.clone())
                    .collect();
                if !writable.is_empty() && !writable.contains(&self.proposal_operation) {
                    self.proposal_operation = writable[0].clone();
                }
                egui::ComboBox::from_label("Operation")
                    .selected_text(if self.proposal_operation.is_empty() {
                        "No write operations available"
                    } else {
                        &self.proposal_operation
                    })
                    .show_ui(ui, |ui| {
                        for name in &writable {
                            ui.selectable_value(&mut self.proposal_operation, name.clone(), name);
                        }
                    });
                ui.label("Arguments (JSON)");
                ui.add(
                    egui::TextEdit::multiline(&mut self.proposal_arguments)
                        .desired_rows(4)
                        .code_editor(),
                );
                let owner_pubkey = self
                    .identities
                    .iter()
                    .find(|identity| identity.name == self.approval_identity)
                    .map(|identity| identity.pubkey_hex.clone());
                if ui
                    .add_enabled(
                        !self.proposal_identity.is_empty()
                            && owner_pubkey.is_some()
                            && !self.proposal_operation.is_empty(),
                        egui::Button::new("Submit for approval"),
                    )
                    .clicked()
                {
                    self.submit_proposal(owner_pubkey.as_deref().unwrap_or_default());
                }
                if let Some(status) = &self.proposal_status {
                    ui.weak(status);
                }
                if let Some(error) = &self.proposal_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), error);
                }

                ui.separator();
                ui.heading("Pending operation requests");
                egui::ComboBox::from_label("Approve as")
                    .selected_text(if self.approval_identity.is_empty() {
                        "Select an identity"
                    } else {
                        &self.approval_identity
                    })
                    .show_ui(ui, |ui| {
                        for identity in &self.identities {
                            ui.selectable_value(
                                &mut self.approval_identity,
                                identity.name.clone(),
                                &identity.name,
                            );
                        }
                    });
                if let Some(status) = &self.approval_status {
                    ui.weak(status);
                }
                if let Some(error) = &self.approval_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), error);
                }
                if self.pending_operations.is_empty() {
                    ui.weak("No pending requests.");
                }
                for pending in self.pending_operations.clone() {
                    ui.group(|ui| {
                        ui.label(egui::RichText::new(&pending.operation).strong());
                        ui.weak(format!(
                            "Actor {} · created {} · expires {} · request published: {}",
                            pending.actor_pubkey.chars().take(12).collect::<String>(),
                            pending.created_at,
                            pending.expires_at,
                            pending.request_published
                        ));
                        if pending.status_source.is_some() {
                            ui.weak(
                                "Offline journal snapshot · decisions require the operation daemon",
                            );
                        }
                        ui.monospace(
                            serde_json::to_string_pretty(&pending.arguments).unwrap_or_default(),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !self.approval_identity.is_empty()
                                        && pending.status_source.is_none(),
                                    egui::Button::new("Approve"),
                                )
                                .clicked()
                            {
                                match operations_client::decide(
                                    &self.approval_identity,
                                    &pending.operation_id,
                                    "approve",
                                ) {
                                    Ok(status) => {
                                        self.approval_status = Some(format!(
                                            "Request {}: {status}",
                                            pending.operation_id
                                        ));
                                        self.approval_error = None;
                                        self.refresh_operations();
                                    }
                                    Err(error) => self.approval_error = Some(error),
                                }
                            }
                            if ui
                                .add_enabled(
                                    !self.approval_identity.is_empty()
                                        && pending.status_source.is_none(),
                                    egui::Button::new("Deny"),
                                )
                                .clicked()
                            {
                                match operations_client::decide(
                                    &self.approval_identity,
                                    &pending.operation_id,
                                    "deny",
                                ) {
                                    Ok(status) => {
                                        self.approval_status = Some(format!(
                                            "Request {}: {status}",
                                            pending.operation_id
                                        ));
                                        self.approval_error = None;
                                        self.refresh_operations();
                                    }
                                    Err(error) => self.approval_error = Some(error),
                                }
                            }
                            if ui
                                .add_enabled(
                                    !self.approval_identity.is_empty()
                                        && pending.status_source.is_none(),
                                    egui::Button::new("Cancel request"),
                                )
                                .clicked()
                            {
                                match operations_client::decide(
                                    &self.approval_identity,
                                    &pending.operation_id,
                                    "cancel",
                                ) {
                                    Ok(status) => {
                                        self.approval_status = Some(format!(
                                            "Request {}: {status}",
                                            pending.operation_id
                                        ));
                                        self.approval_error = None;
                                        self.refresh_operations();
                                    }
                                    Err(error) => self.approval_error = Some(error),
                                }
                            }
                        });
                    });
                }
                ui.collapsing("Recent activity", |ui| {
                    if self.operation_history.is_empty() {
                        ui.weak("No operation requests recorded.");
                    }
                    for entry in &self.operation_history {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&entry.operation).strong());
                                ui.weak(&entry.status);
                            });
                            ui.weak(format!(
                                "Actor {} · operation_id {} · v{} · created {}",
                                entry.actor_pubkey.chars().take(12).collect::<String>(),
                                entry.operation_id,
                                entry.operation_version,
                                entry.created_at
                            ));
                            if entry.status_source.as_deref() == Some("local_journal_offline") {
                                ui.weak("Offline journal snapshot");
                            }
                            ui.weak(format!(
                                "Relay: request={} approval={} execution={} result={}",
                                entry.request_published,
                                entry.approval_published,
                                entry.execution_published,
                                entry.result_published
                            ));
                            if let Some(id) = &entry.approval_event_id {
                                ui.weak(format!("Approval event: {id}"));
                            }
                            if let Some(id) = &entry.execution_event_id {
                                ui.weak(format!("Execution event: {id}"));
                            }
                            if let Some(id) = &entry.result_event_id {
                                ui.weak(format!("Result event: {id}"));
                            }
                            ui.monospace(
                                serde_json::to_string_pretty(&entry.arguments).unwrap_or_default(),
                            );
                            if let Some(result) = &entry.result {
                                ui.label(serde_json::to_string_pretty(result).unwrap_or_default());
                            }
                            if let Some(error) = &entry.error {
                                ui.colored_label(egui::Color32::from_rgb(230, 120, 110), error);
                            }
                        });
                    }
                });

                ui.separator();
                ui.heading("Local file search");
                ui.label("Selected files stay on this device. Supported: .md, .txt, .rst, .org (up to 2 MiB). DMs are not indexed.");
                if ui.button("Find text files in ~/Nostr").clicked() {
                    match ai_index_client::discover_nostr_files() {
                        Ok(files) => {
                            self.ai_nostr_files = files;
                            self.ai_nostr_discovered = true;
                        }
                        Err(error) => self.ai_index_error = Some(error),
                    }
                }
                if self.ai_nostr_discovered && self.ai_nostr_files.is_empty() {
                    ui.weak("No supported text files found in ~/Nostr.");
                }
                for path in self.ai_nostr_files.clone() {
                    ui.horizontal(|ui| {
                        ui.label(&path);
                        if ui.button("Select").clicked() {
                            match ai_index_client::add_file(&path) {
                                Ok(()) => {
                                    self.ai_index_status =
                                        Some("File added to your local index selection.".into());
                                    self.refresh_ai_index();
                                }
                                Err(error) => self.ai_index_error = Some(error),
                            }
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.ai_index_path)
                            .hint_text("Path to a text file"),
                    );
                    if ui.button("Add file").clicked() {
                        match ai_index_client::add_file(&self.ai_index_path) {
                            Ok(()) => {
                                self.ai_index_path.clear();
                                self.ai_index_status =
                                    Some("File added to your local index selection.".into());
                                self.refresh_ai_index();
                            }
                            Err(error) => self.ai_index_error = Some(error),
                        }
                    }
                    if ui.button("Refresh").clicked() {
                        self.refresh_ai_index();
                    }
                });
                if let Some(status) = &self.ai_index_status {
                    ui.weak(status);
                }
                if let Some(error) = &self.ai_index_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), error);
                }
                for scope in self.ai_index_scopes.clone() {
                    ui.horizontal(|ui| {
                        ui.label(&scope.path);
                        if ui.button("Index").clicked() {
                            match ai_index_client::reindex(&scope.id) {
                                Ok((documents, chunks)) => {
                                    self.ai_index_status = Some(format!(
                                        "Indexed {documents} file into {chunks} chunks."
                                    ))
                                }
                                Err(error) => self.ai_index_error = Some(error),
                            }
                        }
                        if ui.button("Remove").clicked() {
                            match ai_index_client::remove_scope(&scope.id) {
                                Ok(()) => match ai_index_client::remove_file(&scope.path) {
                                    Ok(()) => self.refresh_ai_index(),
                                    Err(error) => self.ai_index_error = Some(error),
                                },
                                Err(error) => self.ai_index_error = Some(error),
                            }
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.ai_index_query);
                    if ui.button("Search").clicked() {
                        match ai_index_client::search(&self.ai_index_query) {
                            Ok(results) => {
                                self.ai_index_results = results;
                                self.ai_index_error = None;
                            }
                            Err(error) => self.ai_index_error = Some(error),
                        }
                    }
                });
                for result in &self.ai_index_results {
                    ui.group(|ui| {
                        ui.weak(format!("{} · {:.3}", result.path, result.score));
                        ui.label(&result.excerpt);
                    });
                }

                ui.separator();
                ui.heading("Context sources");
                ui.label("Choose which local context this session may provide to AI.");
                if ui.button("Refresh context sources").clicked() {
                    self.refresh_ai_context();
                }
                for (source, label) in ai_context_client::SOURCES {
                    let mut enabled = self
                        .ai_context_sources
                        .iter()
                        .any(|active| active == source);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut enabled, *label).changed() {
                            let mut updated = self.ai_context_sources.clone();
                            if enabled {
                                updated.push((*source).to_string());
                            } else {
                                updated.retain(|item| item != source);
                            }
                            match ai_context_client::save_sources(&updated) {
                                Ok(()) => match ai_context_client::list_sources() {
                                    Ok(active) => {
                                        self.ai_context_sources = active.clone();
                                        if self.ai_context_data.as_ref().is_some_and(|(source, _)| !active.contains(source)) {
                                            self.ai_context_data = None;
                                        }
                                        self.ai_context_error = (active != updated).then(|| "The context service uses an environment allowlist that overrides the config file.".into());
                                    }
                                    Err(error) => self.ai_context_error = Some(error),
                                },
                                Err(error) => self.ai_context_error = Some(error),
                            }
                        }
                        if enabled && ui.button("Read").clicked() {
                            match ai_context_client::get(source) {
                                Ok(data) => {
                                    self.ai_context_data = Some(((*source).to_string(), data));
                                    self.ai_context_error = None;
                                }
                                Err(error) => self.ai_context_error = Some(error),
                            }
                        }
                    });
                }
                if let Some(error) = &self.ai_context_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), error);
                }
                if let Some((source, data)) = &self.ai_context_data {
                    ui.group(|ui| {
                        ui.weak(source);
                        ui.monospace(serde_json::to_string_pretty(data).unwrap_or_default());
                    });
                }

                if let Some(error) = &self.operations_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), error);
                } else if self.operations.is_empty() {
                    ui.label("No operations available. Start evermore-operationsd to load the catalogue.");
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for operation in &self.operations {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&operation.name).strong());
                                    ui.weak(match operation.access {
                                        evermore_common::operations::Access::Read => "read",
                                        evermore_common::operations::Access::Write => "write",
                                    });
                                    if operation.approval
                                        == evermore_common::operations::Approval::Required
                                    {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(230, 180, 90),
                                            "approval required",
                                        );
                                    }
                                });
                                ui.label(&operation.description);
                                ui.weak(format!("Grant: {}", operation.required_grants.join(", ")));
                            });
                            ui.add_space(6.0);
                        }
                    });
                }
            }
            Tab::Packages => {
                if !self.npack_loaded {
                    self.refresh_npack();
                }
                ui.heading("Packages");
                ui.horizontal(|ui| {
                    ui.label("Identity:");
                    if ui.text_edit_singleline(&mut self.npack_identity).changed() {
                        self.npack_loaded = false;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Blossom URL:");
                    ui.text_edit_singleline(&mut self.npack_blossom_url);
                });
                if ui.button("Refresh").clicked() {
                    self.refresh_npack();
                }
                ui.add_space(8.0);

                if let Some(err) = &self.npack_error {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 110), err);
                    ui.add_space(8.0);
                }

                ui.collapsing("Software Sources (trusted publishers)", |ui| {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.npack_trust_input);
                        if ui.button("Trust").clicked() && !self.npack_identity.trim().is_empty() {
                            match npack_client::trust(
                                &self.npack_identity,
                                self.npack_trust_input.trim(),
                            ) {
                                Ok(()) => {
                                    self.npack_trust_input.clear();
                                    self.refresh_npack();
                                }
                                Err(e) => self.npack_error = Some(e),
                            }
                        }
                    });
                    for pubkey in self.npack_trusted.clone() {
                        ui.horizontal(|ui| {
                            ui.monospace(&pubkey);
                            if ui.small_button("Untrust").clicked() {
                                match npack_client::untrust(&self.npack_identity, &pubkey) {
                                    Ok(()) => self.refresh_npack(),
                                    Err(e) => self.npack_error = Some(e),
                                }
                            }
                        });
                    }
                });
                ui.add_space(8.0);

                ui.heading("Available");
                if self.npack_available.is_empty() {
                    ui.weak("(none -- set an identity and trust a publisher)");
                }
                for pkg in self.npack_available.clone() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&pkg.name).strong());
                            ui.weak(format!("v{}", pkg.version.as_deref().unwrap_or("?")));
                        });
                        ui.weak(format!(
                            "{}  (by {})",
                            pkg.id,
                            short_pubkey(&pkg.publisher_pubkey)
                        ));
                        ui.label(&pkg.description);
                        if ui.button("Install").clicked() {
                            match npack_client::install(
                                &self.npack_identity,
                                &self.npack_blossom_url,
                                &pkg.id,
                            ) {
                                Ok(()) => self.refresh_npack(),
                                Err(e) => self.npack_error = Some(e),
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
                ui.add_space(8.0);

                ui.heading("Installed");
                if self.npack_installed.is_empty() {
                    ui.weak("(nothing installed)");
                }
                for pkg in self.npack_installed.clone() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} v{}", pkg.name, pkg.version));
                        ui.weak(&pkg.id);
                        if ui.small_button("Remove").clicked() {
                            match npack_client::remove(&pkg.id) {
                                Ok(()) => self.refresh_npack(),
                                Err(e) => self.npack_error = Some(e),
                            }
                        }
                    });
                }
            }
        }
    }
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::Local => "[local]",
        Source::Mention => "[mention]",
        Source::Dm => "[dm]",
    }
}

fn short_pubkey(pubkey_hex: &str) -> String {
    if pubkey_hex.len() > 12 {
        format!(
            "{}…{}",
            &pubkey_hex[..6],
            &pubkey_hex[pubkey_hex.len() - 6..]
        )
    } else {
        pubkey_hex.to_string()
    }
}

/// Minimal custom theme so this doesn't look like the default egui demo:
/// darker background, tighter rounding, a touch more breathing room.
fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(18, 18, 22);
    visuals.window_fill = egui::Color32::from_rgb(18, 18, 22);
    visuals.extreme_bg_color = egui::Color32::from_rgb(12, 12, 15);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(24, 24, 29);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(28, 28, 34);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(40, 40, 48);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(90, 70, 220);
    visuals.selection.bg_fill = egui::Color32::from_rgb(90, 70, 220);
    visuals.window_corner_radius = egui::CornerRadius::same(10);
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    ctx.set_style_of(egui::Theme::Dark, style);
}
