mod settings_client;
mod signer_client;

use eframe::egui;
use signer_client::Identity;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([420.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Evermore",
        options,
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(PanelApp::default()))
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
}

impl PanelApp {
    fn refresh_identities(&mut self) {
        self.identities_loaded = true;
        match signer_client::fetch_identities() {
            Ok(identities) => {
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
}

impl eframe::App for PanelApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.selectable_value(&mut self.tab, Tab::Identity, "Identity");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                ui.selectable_value(&mut self.tab, Tab::Notifications, "Notifications");
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
                ui.heading("Notifications");
                ui.label("Nostr + local event feed lands in Phase 3.");
            }
            Tab::Packages => {
                ui.heading("Packages");
                ui.label("npack integration lands in Phase 5.");
            }
        }
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
