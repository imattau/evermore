use eframe::egui;

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
                ui.heading("Identity");
                ui.label("No evermore-signer connection yet.");
                ui.add_space(8.0);
                ui.label("This is where loaded identities and their npubs will show up once the signing daemon (Phase 1) is wired in.");
            }
            Tab::Settings => {
                ui.heading("Settings");
                ui.label("NIP-78 sync lands in Phase 2.");
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
