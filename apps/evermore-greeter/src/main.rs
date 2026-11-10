//! `evermore-greeter`: a custom greetd frontend (run inside a `cage`
//! kiosk Wayland session, per `packaging/greetd/config.toml`) showing
//! each provisioned identity's `kind:0`-derived display name and
//! avatar at login, instead of a bare username prompt. Authentication
//! itself is entirely greetd's/PAM's job (`pam_systemd_home` already
//! does the homed-aware work, unmodified, as a stock Arch install --
//! see PROJECT_IDEA.md's Phase 5.5); this binary only ever relays
//! prompts, never touches a password itself beyond passing it through.

mod greetd_client;
mod homed;
mod profile;

use eframe::egui;
use egui::{Color32, ColorImage, TextureHandle};
use greetd_client::{AuthStep, GreetdClient};
use nostr::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use zeroize::{Zeroize, Zeroizing};

/// What a stock greetd frontend would also need to decide: which
/// command actually starts the real desktop session once auth
/// succeeds. Evermore's own answer -- Sway, with whatever the user's
/// own `~/.config/sway/config` includes (see
/// `packaging/sway/install-desktop.sh`) -- since that's this project's
/// one supported desktop, not a configurable session picker.
const SESSION_CMD: &[&str] = &["/usr/bin/sway"];

/// "Green is rare. Only signed, installed, or synced confirmation" per the
/// Evermore Graphite design note -- there's no `egui::Visuals` slot for it
/// (unlike warn/error, which `apply_theme` below sets), so the one place
/// that needs it (phone-pairing confirmation) gets its own constant.
const SUCCESS: Color32 = Color32::from_rgb(95, 191, 143);

/// Tokens from the "Evermore Desktop — UI improvements" Figma file's
/// first-boot wizard frames, which refine a few of `build_visuals`'s dark
/// tuples and add the ones that file introduces (the ink-800 "raised
/// surface" tier the design note always named but no code ever assigned a
/// literal value to, a faint fourth text tier, an accent-tinted highlight
/// chip, and the near-black text color used on filled-accent surfaces).
mod setup_tokens {
    use super::Color32;

    pub const CARD_BG: Color32 = Color32::from_rgb(20, 23, 29); // #14171D (= ink-900/chrome)
    pub const RAISED: Color32 = Color32::from_rgb(33, 38, 47); // #21262F
    pub const LINE: Color32 = Color32::from_rgb(48, 55, 67); // #303743
    pub const TEXT_FAINT: Color32 = Color32::from_rgb(126, 137, 153); // #7E8999
    pub const ACCENT: Color32 = Color32::from_rgb(110, 168, 254); // #6EA8FE
    pub const ACCENT_INK: Color32 = Color32::from_rgb(7, 9, 12); // #07090C
    pub const ACCENT_CHIP: Color32 = Color32::from_rgb(38, 70, 120); // #264678

    pub const CARD_MAX_WIDTH: f32 = 1080.0;
    pub const SIDEBAR_WIDTH: f32 = 230.0;
    pub const STEP_ROW_HEIGHT: f32 = 46.0;
    pub const STEP_CIRCLE_SIZE: f32 = 26.0;
}
use setup_tokens::{
    ACCENT as SETUP_ACCENT, ACCENT_CHIP, ACCENT_INK, CARD_BG, CARD_MAX_WIDTH, LINE, RAISED,
    SIDEBAR_WIDTH, STEP_CIRCLE_SIZE, STEP_ROW_HEIGHT, TEXT_FAINT,
};

/// The step sidebar's connector line and circles, painted directly (same
/// idiom as `nav_item`/`command_palette_row`) rather than built from
/// stacked widgets: the connector has to span from the first circle's
/// center to the last, which needs the geometry decided up front.
fn setup_sidebar(ui: &mut egui::Ui, current_step: usize) {
    const STEPS: [&str; SETUP_TOTAL_STEPS] = [
        "Welcome",
        "Account details",
        "Identity",
        "Recovery key",
        "Pair phone (optional)",
        "Passphrase",
    ];
    ui.label(
        egui::RichText::new("YOUR SETUP")
            .size(11.0)
            .strong()
            .color(ui.visuals().weak_text_color.unwrap_or(Color32::GRAY)),
    );
    ui.add_space(14.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), STEP_ROW_HEIGHT * STEPS.len() as f32),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let circle_x = rect.left() + STEP_CIRCLE_SIZE / 2.0;
    let row_center = |index: usize| rect.top() + STEP_ROW_HEIGHT * (index as f32 + 0.5);
    painter.line_segment(
        [
            egui::pos2(circle_x, row_center(0)),
            egui::pos2(circle_x, row_center(STEPS.len() - 1)),
        ],
        egui::Stroke::new(2.0, LINE),
    );
    for (index, label) in STEPS.iter().enumerate() {
        let step_number = index + 1;
        let center = egui::pos2(circle_x, row_center(index));
        if step_number <= current_step {
            painter.circle_filled(center, STEP_CIRCLE_SIZE / 2.0, SETUP_ACCENT);
            let glyph = if step_number < current_step {
                "\u{2713}".to_owned()
            } else {
                step_number.to_string()
            };
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(12.0),
                ACCENT_INK,
            );
        } else {
            painter.circle_stroke(center, STEP_CIRCLE_SIZE / 2.0, egui::Stroke::new(1.0, LINE));
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                step_number.to_string(),
                egui::FontId::proportional(12.0),
                ui.visuals().weak_text_color.unwrap_or(Color32::GRAY),
            );
        }
        let label_pos = egui::pos2(rect.left() + STEP_CIRCLE_SIZE + 14.0, center.y);
        let (color, strong) = match step_number.cmp(&current_step) {
            std::cmp::Ordering::Equal => (ui.visuals().strong_text_color(), true),
            std::cmp::Ordering::Less => (ui.visuals().weak_text_color.unwrap_or(Color32::GRAY), false),
            std::cmp::Ordering::Greater => (TEXT_FAINT, false),
        };
        let font = if strong {
            egui::FontId::new(13.0, egui::FontFamily::Name("inter-semibold".into()))
        } else {
            egui::FontId::proportional(13.0)
        };
        painter.text(label_pos, egui::Align2::LEFT_CENTER, *label, font, color);
    }
    ui.add_space(28.0);
    ui.label(
        egui::RichText::new("Your keys stay under your control.")
            .size(13.0)
            .color(ui.visuals().weak_text_color.unwrap_or(Color32::GRAY)),
    );
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("You can finish setup without network access.")
            .size(12.0)
            .color(TEXT_FAINT),
    );
}

/// The "STEP N OF 6" chip above each step's heading.
fn setup_step_pill(ui: &mut egui::Ui, current_step: usize) {
    let text = format!("STEP {current_step} OF {SETUP_TOTAL_STEPS}");
    let galley =
        ui.painter()
            .layout_no_wrap(text.clone(), egui::FontId::proportional(12.0), SETUP_ACCENT);
    let size = egui::vec2(galley.size().x + 28.0, 26.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, size.y / 2.0, ACCENT_CHIP);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(12.0),
        SETUP_ACCENT,
    );
}

/// The two info-box styles the design uses throughout the wizard: a
/// neutral bordered box for general explanation, and a flat
/// accent-tinted one (no border) for "pay attention to this" -- the
/// Identity and Recovery steps' safety notices. Same shape, different
/// fill, so one function covers both via `accent`.
fn setup_callout(ui: &mut egui::Ui, title: &str, body: &str, accent: bool) {
    let fill = if accent { ACCENT_CHIP } else { RAISED };
    let frame = egui::Frame::new()
        .fill(fill)
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(16, 14));
    let frame = if accent {
        frame
    } else {
        frame.stroke(egui::Stroke::new(1.0, LINE))
    };
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(title).strong().size(14.0));
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(body)
                .size(12.5)
                .color(ui.visuals().weak_text_color.unwrap_or(Color32::GRAY)),
        );
    });
}

/// A filled accent call-to-action button -- the wizard's one primary
/// action per step. Distinct from the plain `ui.button`, which this
/// theme renders flat/borderless (right for secondary actions like "Skip
/// for now", wrong for the one button each step actually wants pressed).
fn setup_primary_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong().color(ACCENT_INK))
            .fill(SETUP_ACCENT)
            .corner_radius(10.0)
            .min_size(egui::vec2(0.0, 44.0)),
    )
    .clicked()
}

/// A neutral bordered button -- "Skip for now" and the like. The
/// `ui.button` default in this theme is flat/borderless; the wizard's
/// secondary actions are drawn as a raised, bordered surface instead.
fn setup_secondary_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(RAISED)
            .stroke(egui::Stroke::new(1.0, LINE))
            .corner_radius(10.0)
            .min_size(egui::vec2(0.0, 44.0)),
    )
    .clicked()
}

/// A labeled field: bold label, full-width input, optional muted helper
/// line -- the pattern every text field in the redesigned wizard uses,
/// replacing the previous single `ui.label` + bare `TextEdit`.
fn setup_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    password: bool,
    helper: Option<&str>,
) {
    ui.label(egui::RichText::new(label).strong().size(13.0));
    ui.add_space(6.0);
    ui.add(
        egui::TextEdit::singleline(value)
            .password(password)
            .hint_text(hint)
            .desired_width(ui.available_width())
            .min_size(egui::vec2(0.0, 44.0)),
    );
    if let Some(helper) = helper {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(helper)
                .size(11.5)
                .color(ui.visuals().weak_text_color.unwrap_or(Color32::GRAY)),
        );
    }
}

/// Evermore Graphite dark palette and type scale, kept in lockstep with
/// `apps/evermore-panel`'s `build_visuals`/`build_style`/`load_fonts` (the
/// design note's source-of-truth implementation) -- the greeter is a kiosk
/// lock screen with no light/dark toggle, so only the dark branch is needed.
fn build_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();

    let canvas = Color32::from_rgb(14, 16, 20); // ink-950: "application canvas, lock screen"
    let chrome = Color32::from_rgb(20, 23, 29); // ink-900
    let card = Color32::from_rgb(26, 30, 37); // ink-850
    let text = Color32::from_rgb(230, 233, 239);
    let text_muted = Color32::from_rgb(167, 176, 191);
    let accent = Color32::from_rgb(110, 168, 254);
    let accent_strong = Color32::from_rgb(139, 183, 255);
    // The Figma "Evermore Desktop — UI improvements" file is this app's
    // newer source of truth for a few tokens the design note always named
    // but never assigned a literal value in code: `ink-800` ("inputs,
    // raised surfaces") turns out to be #21262F, and the hairline border
    // around those surfaces is #303743 -- both reused from
    // `setup_tokens` so this function and the wizard draw from one place.
    let raised = setup_tokens::RAISED;
    let line = setup_tokens::LINE;

    // Unlike `evermore-panel` (which has a separate chrome layer around its
    // content), this whole screen *is* the lock screen the design note
    // calls out `ink-950` for -- so the full-bleed `CentralPanel` fill is
    // the canvas tone, not the one-step-up panel-chrome tone. Text inputs
    // sit a tier above that (now `ink-800`/`raised`, per the Figma file)
    // to read as a raised surface at all.
    visuals.panel_fill = canvas;
    visuals.window_fill = chrome;
    visuals.extreme_bg_color = raised;
    visuals.faint_bg_color = card;
    visuals.weak_text_color = Some(text_muted);
    visuals.override_text_color = Some(text);
    visuals.hyperlink_color = accent;
    visuals.warn_fg_color = Color32::from_rgb(229, 181, 103);
    visuals.error_fg_color = Color32::from_rgb(224, 108, 117);
    visuals.selection.bg_fill = accent;
    visuals.selection.stroke = egui::Stroke::new(1.0, accent);

    visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, text);
    // Flat/borderless was the original minimal take; the Figma file draws
    // every input and secondary button with a visible hairline border, so
    // inactive widgets (which TextEdit's unfocused state uses) get one.
    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, line);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, text);
    // "Blue means interactive": hover/press feedback shares the accent,
    // same as every other Evermore egui surface.
    visuals.widgets.hovered.bg_fill = accent_strong.gamma_multiply(0.55);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, text);
    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, text);
    visuals.window_corner_radius = egui::CornerRadius::same(12);
    visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(12);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(12);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(12);
    visuals
}

fn build_style(base: &egui::Style) -> egui::Style {
    let mut style = base.clone();
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.window_margin = egui::Margin::same(16);
    let inter_medium = egui::FontFamily::Name("inter-medium".into());
    let inter_semibold = egui::FontFamily::Name("inter-semibold".into());
    style.text_styles = [
        (
            egui::TextStyle::Small,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Body,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Button,
            egui::FontId::new(13.0, inter_medium),
        ),
        (
            egui::TextStyle::Heading,
            egui::FontId::new(18.0, inter_semibold),
        ),
        (
            egui::TextStyle::Monospace,
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
        ),
    ]
    .into();
    style
}

fn apply_theme(ctx: &egui::Context) {
    // Kiosk lock screen, so only `Theme::Dark` is ever shown -- but egui's
    // visuals/style are still namespaced per theme, so both calls are
    // required even though `Theme::Light` is never selected.
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.set_visuals_of(theme, build_visuals());
        let style = build_style(&ctx.style_of(theme));
        ctx.set_style_of(theme, style);
    }
    ctx.set_theme(egui::ThemePreference::Dark);
}

/// Bundles the same Inter + JetBrains Mono files `evermore-panel` ships,
/// rather than duplicating the font binaries in this crate too.
fn load_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "inter-regular".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../evermore-panel/assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-medium".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../evermore-panel/assets/fonts/Inter-Medium.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semibold".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../evermore-panel/assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "jetbrains-mono".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../evermore-panel/assets/fonts/JetBrainsMono-Regular.ttf"
        ))),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "inter-regular".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains-mono".to_owned());
    fonts.families.insert(
        egui::FontFamily::Name("inter-medium".into()),
        vec!["inter-medium".to_owned()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("inter-semibold".into()),
        vec!["inter-semibold".to_owned()],
    );

    ctx.set_fonts(fonts);
}

struct UserEntry {
    user_name: String,
    display_name: String,
    avatar_path: Option<PathBuf>,
}

enum Screen {
    FirstUser(FirstUserSetup),
    /// `error` is shown above the picker when a previous login attempt
    /// failed or was cancelled -- e.g. a wrong password lands back
    /// here rather than re-prompting blind.
    Picker {
        error: Option<String>,
    },
    Auth {
        user_name: String,
        prompt: String,
        secret: bool,
        input: String,
        /// Whether the password field has already been given focus
        /// once. Requesting it fresh on *every* frame (instead of
        /// just the first one) re-grabbed focus back onto the field
        /// the instant Enter surrendered it to submit, silently
        /// defeating every way to actually submit the form -- a real
        /// bug found live-testing on the VM, not a hypothetical.
        focus_requested: bool,
    },
    Starting,
    /// Something unrelated to a specific login attempt went wrong
    /// (can't reach greetd's socket, can't talk to homed at all) --
    /// there is no sensible picker to fall back to.
    FatalError(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SetupStep {
    Welcome,
    Details,
    Identity,
    Pairing,
    Passphrase,
    Recovery,
    Saving,
}

/// Total step count for the wizard's progress indicator, fixed at the
/// *longer* of its two paths (generating a new identity visits
/// `Recovery`; importing an existing `nsec` skips straight from
/// `Identity` to `Pairing`). Wizard UX guidance is consistent that a
/// step count which can grow mid-flow (e.g. "step 2 of 5" becoming
/// "step 2 of 6") damages trust more than a fixed total with one step
/// skipped ever does, so the import path's indicator jumps 3 -> 5
/// rather than renumbering down to a count of 5.
const SETUP_TOTAL_STEPS: usize = 6;

/// `None` for `Saving`, which isn't a step the user acts on -- it's a
/// transient processing state with its own "Creating your encrypted
/// home…" message, not something to count toward "done".
fn setup_step_number(step: SetupStep) -> Option<usize> {
    match step {
        SetupStep::Welcome => Some(1),
        SetupStep::Details => Some(2),
        SetupStep::Identity => Some(3),
        SetupStep::Recovery => Some(4),
        SetupStep::Pairing => Some(5),
        SetupStep::Passphrase => Some(6),
        SetupStep::Saving => None,
    }
}

struct FirstUserSetup {
    step: SetupStep,
    username: String,
    display_name: String,
    imported_nsec: String,
    identity_secret: Option<Zeroizing<String>>,
    generated: bool,
    passphrase: String,
    confirmation: String,
    error: Option<String>,
    relay: String,
    pairing_ready: bool,
    paired_user_pubkey: Option<String>,
    pair_rx: Option<Receiver<String>>,
    pair_cancel: Option<mpsc::Sender<()>>,
    pair_uri: Option<String>,
    pair_qr: Option<TextureHandle>,
}

impl FirstUserSetup {
    fn new() -> Self {
        Self {
            step: SetupStep::Welcome,
            username: String::new(),
            display_name: String::new(),
            imported_nsec: String::new(),
            identity_secret: None,
            generated: false,
            passphrase: String::new(),
            confirmation: String::new(),
            error: None,
            relay: "wss://relay.damus.io".to_owned(),
            pairing_ready: false,
            paired_user_pubkey: None,
            pair_rx: None,
            pair_cancel: None,
            pair_uri: None,
            pair_qr: None,
        }
    }
    fn prepare_identity(&mut self) -> Result<(), String> {
        let key = if self.imported_nsec.trim().is_empty() {
            self.generated = true;
            SecretKey::generate()
        } else {
            self.generated = false;
            SecretKey::parse(self.imported_nsec.trim())
                .map_err(|e| format!("That nsec is invalid: {e}"))?
        };
        self.identity_secret = Some(Zeroizing::new(key.to_secret_hex()));
        self.step = if self.generated {
            SetupStep::Recovery
        } else {
            SetupStep::Pairing
        };
        Ok(())
    }
    fn create(&mut self) -> Result<(), String> {
        let secret = self.identity_secret.as_ref().ok_or("identity is missing")?;
        #[derive(serde::Serialize)]
        struct Request<'a> {
            username: &'a str,
            display_name: &'a str,
            identity_secret: &'a str,
            passphrase: &'a str,
            pairing_enabled: bool,
        }
        let payload = Zeroizing::new(
            serde_json::to_string(&Request {
                username: self.username.trim(),
                display_name: self.display_name.trim(),
                identity_secret: secret.as_str(),
                passphrase: &self.passphrase,
                pairing_enabled: self.pairing_ready,
            })
            .map_err(|e| e.to_string())?,
        );
        let mut child = std::process::Command::new("sudo")
            .args(["-n", "/usr/bin/evermore-enroll", "first-user-create"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start account setup: {e}"))?;
        child
            .stdin
            .as_mut()
            .ok_or("account setup input unavailable")?
            .write_all(payload.as_bytes())
            .map_err(|e| e.to_string())?;
        drop(child.stdin.take());
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(())
    }

    fn start_pairing(&mut self, ctx: egui::Context) -> Result<(), String> {
        if self.pair_rx.is_some() {
            return Ok(());
        }
        let mut child = std::process::Command::new("sudo")
            .args(["-n", "/usr/bin/evermore-enroll", "first-user-pair"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start phone pairing: {e}"))?;
        writeln!(
            child.stdin.as_mut().ok_or("pairing input unavailable")?,
            "{}",
            self.relay.trim()
        )
        .map_err(|e| e.to_string())?;
        drop(child.stdin.take());
        let stdout = child.stdout.take().ok_or("pairing output unavailable")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("pairing error output unavailable")?;
        let (tx, rx) = mpsc::channel();
        let (cancel_tx, cancel_rx) = mpsc::channel();
        std::thread::spawn(move || {
            use std::io::{BufRead, Read};
            let (line_tx, line_rx) = mpsc::channel();
            let reader_thread = std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout).lines().flatten() {
                    if line_tx.send(line).is_err() {
                        break;
                    }
                }
            });
            let mut stderr = stderr;
            loop {
                if cancel_rx.try_recv().is_ok() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                while let Ok(line) = line_rx.try_recv() {
                    if tx.send(line).is_err() {
                        let _ = child.kill();
                        return;
                    }
                    ctx.request_repaint();
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let _ = reader_thread.join();
                        while let Ok(line) = line_rx.try_recv() {
                            let _ = tx.send(line);
                        }
                        let mut message = String::new();
                        let _ = stderr.read_to_string(&mut message);
                        if !status.success() {
                            let _ = tx.send(
                                serde_json::json!({"state":"error", "message":message.trim()})
                                    .to_string(),
                            );
                            ctx.request_repaint();
                        }
                        return;
                    }
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
                    Err(error) => {
                        let _ = child.kill();
                        let _ = tx.send(
                            serde_json::json!({"state":"error", "message":error.to_string()})
                                .to_string(),
                        );
                        ctx.request_repaint();
                        return;
                    }
                }
            }
        });
        self.pair_rx = Some(rx);
        self.pair_cancel = Some(cancel_tx);
        self.error = None;
        Ok(())
    }
}

impl Drop for FirstUserSetup {
    fn drop(&mut self) {
        self.imported_nsec.zeroize();
        self.passphrase.zeroize();
        self.confirmation.zeroize();
        if let Some(uri) = &mut self.pair_uri {
            uri.zeroize();
        }
    }
}

struct GreeterApp {
    users: Vec<UserEntry>,
    avatars: HashMap<String, Option<TextureHandle>>,
    screen: Screen,
    client: Option<GreetdClient>,
    background: Option<TextureHandle>,
}

impl GreeterApp {
    fn new(background: Option<TextureHandle>) -> Self {
        match homed::list_users() {
            Ok(entries) => {
                let has_users = entries.iter().any(|u| u.state != "absent");
                let users = entries
                    .into_iter()
                    .filter(|u| u.state != "absent")
                    .map(|u| {
                        let cached = u.pubkey_hex.as_deref().and_then(profile::load);
                        UserEntry {
                            display_name: cached
                                .as_ref()
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| u.user_name.clone()),
                            avatar_path: cached.and_then(|p| p.avatar_path),
                            user_name: u.user_name,
                        }
                    })
                    .collect();
                Self {
                    users,
                    avatars: HashMap::new(),
                    screen: if has_users {
                        Screen::Picker { error: None }
                    } else {
                        Screen::FirstUser(FirstUserSetup::new())
                    },
                    client: None,
                    background,
                }
            }
            Err(e) => Self {
                users: Vec::new(),
                avatars: HashMap::new(),
                screen: Screen::FatalError(format!("listing homed users failed: {e}")),
                client: None,
                background,
            },
        }
    }

    fn avatar_texture(&mut self, ctx: &egui::Context, user_name: &str) -> Option<TextureHandle> {
        if let Some(cached) = self.avatars.get(user_name) {
            return cached.clone();
        }
        let path = self
            .users
            .iter()
            .find(|u| u.user_name == user_name)
            .and_then(|u| u.avatar_path.as_ref());
        let texture = path.and_then(|path| load_avatar_texture(ctx, path));
        self.avatars.insert(user_name.to_string(), texture.clone());
        texture
    }

    fn select_user(&mut self, user_name: &str) {
        let mut client = match GreetdClient::connect() {
            Ok(c) => c,
            Err(e) => {
                self.screen = Screen::FatalError(e);
                return;
            }
        };
        let step = client.create_session(user_name);
        self.handle_step(client, user_name, step);
    }

    /// Submits `response` to the session currently in progress and
    /// advances to whatever screen the result calls for.
    fn submit(&mut self, user_name: &str, response: Option<&str>) {
        let Some(mut client) = self.client.take() else {
            self.screen = Screen::FatalError("lost connection to greetd".to_string());
            return;
        };
        let step = client.respond(response);
        self.handle_step(client, user_name, step);
    }

    fn handle_step(
        &mut self,
        mut client: GreetdClient,
        user_name: &str,
        step: Result<AuthStep, String>,
    ) {
        match step {
            Ok(AuthStep::Prompt { message, secret }) => {
                self.client = Some(client);
                self.screen = Screen::Auth {
                    user_name: user_name.to_string(),
                    prompt: message,
                    secret,
                    input: String::new(),
                    focus_requested: false,
                };
            }
            // Informational/error PAM messages need no answer -- just
            // log and acknowledge, then keep going through the same loop.
            Ok(AuthStep::Notice { message, is_error }) => {
                eprintln!(
                    "evermore-greeter: PAM {}: {message}",
                    if is_error { "error" } else { "notice" }
                );
                let step = client.respond(None);
                self.handle_step(client, user_name, step);
            }
            Ok(AuthStep::Done) => self.start_session(client),
            Err(e) => {
                client.cancel_session();
                self.screen = Screen::Picker { error: Some(e) };
            }
        }
    }

    fn start_session(&mut self, client: GreetdClient) {
        self.screen = Screen::Starting;
        match client.start_session(SESSION_CMD, &[]) {
            // greetd only actually starts the session once this
            // process exits -- see greetd_client::GreetdClient::start_session.
            Ok(()) => std::process::exit(0),
            Err(e) => self.screen = Screen::FatalError(e),
        }
    }
}

/// The "graphite" wallpaper variant (`packaging/wallpaper/`, generated by
/// `packaging/wallpaper/generate.py` from this app's own `build_visuals`
/// tuples) as the lock-screen/first-boot background -- the neutral default
/// since neither screen has a signed-in identity's appearance settings
/// (`dusk`/`frost` are user-facing desktop-wallpaper choices, not relevant
/// pre-login). Same cross-crate `include_bytes!` convention as the bundled
/// fonts and `mcp_agent.rs`'s grammar file.
fn load_background_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let bytes: &[u8] = include_bytes!("../../../packaging/wallpaper/graphite.png");
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = image.dimensions();
    let color_image = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &image);
    Some(ctx.load_texture(
        "greeter-background",
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}

/// UV rect that crops (never stretches) `image` to cover a `target`
/// rect of a different aspect ratio, the same "fill"/"cover" behavior
/// `swaybg -m fill` gives the desktop wallpaper -- naively stretching
/// the UV to 0..1 on both axes would visibly distort the image on any
/// screen whose aspect ratio isn't exactly the source PNG's.
fn cover_uv(image_size: egui::Vec2, target_size: egui::Vec2) -> egui::Rect {
    let image_aspect = image_size.x / image_size.y;
    let target_aspect = target_size.x / target_size.y;
    if target_aspect > image_aspect {
        // Target is relatively wider than the image: fill the width,
        // crop the height.
        let visible_h = image_aspect / target_aspect;
        let v0 = (1.0 - visible_h) / 2.0;
        egui::Rect::from_min_max(egui::pos2(0.0, v0), egui::pos2(1.0, 1.0 - v0))
    } else {
        // Target is relatively taller/narrower: fill the height, crop
        // the width.
        let visible_w = target_aspect / image_aspect;
        let u0 = (1.0 - visible_w) / 2.0;
        egui::Rect::from_min_max(egui::pos2(u0, 0.0), egui::pos2(1.0 - u0, 1.0))
    }
}

fn load_avatar_texture(ctx: &egui::Context, path: &std::path::Path) -> Option<TextureHandle> {
    let bytes = std::fs::read(path).ok()?;
    let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = image.dimensions();
    let color_image = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &image);
    Some(ctx.load_texture(
        path.to_string_lossy(),
        color_image,
        egui::TextureOptions::LINEAR,
    ))
}

fn make_qr_texture(ctx: &egui::Context, uri: &str) -> Option<TextureHandle> {
    let mut child = std::process::Command::new("qrencode")
        .args(["-t", "PNG", "-o", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(uri.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let image = image::load_from_memory(&output.stdout).ok()?.to_rgba8();
    let (w, h) = image.dimensions();
    Some(ctx.load_texture(
        "nip46-pairing-qr",
        ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &image),
        egui::TextureOptions::NEAREST,
    ))
}

/// Left padding that centers a row of buttons inside a `horizontal`
/// nested in `vertical_centered` -- that combination doesn't center on
/// its own in this egui version (see the avatar row below, which hit
/// the same thing with fixed-size tiles). Button widths aren't fixed
/// here, so they're estimated from each label's laid-out text size
/// plus the theme's button padding, which is exactly what egui uses
/// internally to size a `Button`.
fn centered_row_padding(ui: &egui::Ui, labels: &[&str]) -> f32 {
    let spacing = ui.spacing().item_spacing.x;
    let button_padding = ui.spacing().button_padding;
    let total_width: f32 = labels
        .iter()
        .map(|label| {
            let galley = ui.painter().layout_no_wrap(
                label.to_string(),
                egui::FontId::default(),
                Color32::WHITE,
            );
            galley.size().x + button_padding.x * 2.0
        })
        .sum::<f32>()
        + spacing * labels.len().saturating_sub(1) as f32;
    ((ui.available_width() - total_width) / 2.0).max(0.0)
}

fn initial_avatar(ui: &mut egui::Ui, name: &str, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let painter = ui.painter();
    let letter = name.chars().next().unwrap_or('?').to_ascii_uppercase();
    painter.circle_filled(rect.center(), size / 2.0, initial_color(name));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional(size * 0.45),
        Color32::WHITE,
    );
    response
}

fn initial_color(name: &str) -> Color32 {
    let hash: u32 = name
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33).wrapping_add(b as u32));
    let hue = (hash % 360) as f32 / 360.0;
    let rgb = egui::ecolor::Hsva::new(hue, 0.55, 0.55, 1.0).to_rgb();
    Color32::from_rgb(
        (rgb[0] * 255.0) as u8,
        (rgb[1] * 255.0) as u8,
        (rgb[2] * 255.0) as u8,
    )
}

impl eframe::App for GreeterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // The root `Ui` eframe hands `App::ui` has no fixed width of
        // its own to center against (see its doc comment: "no margin
        // or background" -- wrap in a panel). Without this, every
        // `vertical_centered` below silently computed its centering
        // against the wrong bounds and the whole picker row rendered
        // stuck at the left edge -- confirmed visually on the VM, not
        // a hypothetical.
        if let Some(background) = &self.background {
            let screen_rect = ui.max_rect();
            let uv = cover_uv(
                egui::vec2(background.size()[0] as f32, background.size()[1] as f32),
                screen_rect.size(),
            );
            ui.painter()
                .image(background.id(), screen_rect, uv, Color32::WHITE);
        }
        // Transparent fill so the background image painted above shows
        // through -- CentralPanel's own fill would otherwise paint
        // straight over it.
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(Color32::TRANSPARENT))
            .show(ui, |ui| {
            match std::mem::replace(&mut self.screen, Screen::Starting) {
                Screen::FirstUser(setup) => self.ui_first_user(ui, setup),
                Screen::Picker { error } => self.ui_picker(ui, &ctx, error),
                Screen::Auth {
                    user_name,
                    prompt,
                    secret,
                    input,
                    focus_requested,
                } => self.ui_auth(ui, user_name, prompt, secret, input, focus_requested),
                Screen::Starting => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(200.0);
                        ui.label("Starting session...");
                    });
                    self.screen = Screen::Starting;
                }
                Screen::FatalError(error) => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.colored_label(ui.visuals().error_fg_color, "evermore-greeter error");
                        ui.label(&error);
                    });
                    self.screen = Screen::FatalError(error);
                }
            }
        });
        ctx.request_repaint();
    }
}

impl GreeterApp {
    fn ui_first_user(&mut self, ui: &mut egui::Ui, mut setup: FirstUserSetup) {
        let mut next = None;
        let mut create = false;
        let highlight_step = setup_step_number(setup.step).unwrap_or(SETUP_TOTAL_STEPS);
        let muted = ui.visuals().weak_text_color.unwrap_or(Color32::GRAY);

        let available_width = ui.available_width();
        let card_width = CARD_MAX_WIDTH.min((available_width - 64.0).max(480.0));
        let margin = ((available_width - card_width) / 2.0).max(24.0);
        let card_inner_margin = 32.0;
        let column_gap = 32.0;
        let content_width =
            (card_width - card_inner_margin * 2.0 - SIDEBAR_WIDTH - column_gap).max(300.0);

        ui.add_space(32.0);
        ui.horizontal(|ui| {
            ui.add_space(margin);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let (mark_rect, _) =
                        ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(mark_rect.center(), 12.0, SETUP_ACCENT);
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("evermore").strong().size(16.0));
                });
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Your identity and encrypted home.").color(muted));
            });
        });
        ui.add_space(24.0);

        ui.horizontal(|ui| {
            ui.add_space(margin);
            egui::Frame::new()
                .fill(CARD_BG)
                .corner_radius(18.0)
                .inner_margin(egui::Margin::symmetric(card_inner_margin as i8, card_inner_margin as i8))
                .show(ui, |ui| {
                    ui.set_width(card_width - card_inner_margin * 2.0);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(SIDEBAR_WIDTH);
                            setup_sidebar(ui, highlight_step);
                        });
                        ui.add_space(column_gap);
                        ui.vertical(|ui| {
                            ui.set_width(content_width);
                            setup_step_pill(ui, highlight_step);
                            ui.add_space(10.0);
                            match setup.step {
                                SetupStep::Welcome => {
                                    ui.label(egui::RichText::new("Set up Evermore").size(24.0).strong());
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new("Create your first account and encrypted home. You can create a Nostr identity or import one you already own.").size(14.0).color(muted));
                                    ui.add_space(20.0);
                                    setup_callout(ui, "Works offline", "A home passphrase will always unlock this account, even without network access.", false);
                                    ui.add_space(12.0);
                                    setup_callout(ui, "Phone approval is optional", "Connect a phone later for NIP-46 approval. Your passphrase remains available as a fallback.", false);
                                    ui.add_space(20.0);
                                    ui.horizontal(|ui| {
                                        if setup_primary_button(ui, "Begin setup") { next = Some(SetupStep::Details); }
                                        ui.add_space(16.0);
                                        ui.label(egui::RichText::new("Next, choose your account name and display name.").size(13.0).color(TEXT_FAINT));
                                    });
                                }
                                SetupStep::Details => {
                                    ui.label(egui::RichText::new("Choose your account details").size(24.0).strong());
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new("Choose the account name used on this device and the name shown to people.").size(14.0).color(muted));
                                    ui.add_space(20.0);
                                    setup_field(ui, "Account name", &mut setup.username, "e.g. alex", false, Some("1–31 lowercase letters, digits, _ or -. Start with a letter or _."));
                                    ui.add_space(16.0);
                                    setup_field(ui, "Display name", &mut setup.display_name, "Name shown on your profile", false, Some("This name appears on your profile."));
                                    ui.add_space(20.0);
                                    if setup_primary_button(ui, "Continue") {
                                        let name = setup.username.trim();
                                        let valid = !name.is_empty() && name.len() <= 31 && (name.as_bytes()[0].is_ascii_lowercase() || name.as_bytes()[0] == b'_') && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
                                        if !valid { setup.error = Some("Use 1–31 lowercase letters, digits, _ or -, starting with a letter or _".into()); }
                                        else if setup.display_name.trim().is_empty() { setup.error = Some("Enter a display name.".into()); }
                                        else { setup.error = None; next = Some(SetupStep::Identity); }
                                    }
                                }
                                SetupStep::Identity => {
                                    ui.label(egui::RichText::new("Create or import a Nostr identity").size(24.0).strong());
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new("Create a new identity, or import an existing secret key (nsec).").size(14.0).color(muted));
                                    ui.add_space(20.0);
                                    setup_field(ui, "Existing nsec (optional)", &mut setup.imported_nsec, "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}", true, Some("Leave this field empty to create a new identity. New keys are shown in the next step."));
                                    ui.add_space(16.0);
                                    setup_callout(ui, "Your secret stays on this device", "Importing an existing identity skips the new-key recovery step.", true);
                                    ui.add_space(20.0);
                                    if setup_primary_button(ui, "Continue") { if let Err(e) = setup.prepare_identity() { setup.error = Some(e); } }
                                }
                                SetupStep::Recovery => {
                                    let recovery = setup.identity_secret.as_ref().and_then(|secret| SecretKey::parse(secret.as_str()).ok()).and_then(|key| key.to_bech32().ok()).unwrap_or_default();
                                    ui.label(egui::RichText::new("Save your recovery key").size(24.0).strong());
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new("Save your recovery key somewhere private. It is shown once.").size(14.0).color(muted));
                                    ui.add_space(20.0);
                                    egui::Frame::new()
                                        .fill(RAISED)
                                        .stroke(egui::Stroke::new(1.0, LINE))
                                        .corner_radius(12.0)
                                        .inner_margin(egui::Margin::symmetric(16, 14))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.label(egui::RichText::new("RECOVERY KEY \u{b7} SHOWN ONCE").size(11.0).strong().color(muted));
                                            ui.add_space(10.0);
                                            ui.add(egui::TextEdit::singleline(&mut recovery.clone()).desired_width(ui.available_width()).interactive(false).font(egui::FontId::monospace(14.0)));
                                        });
                                    ui.add_space(8.0);
                                    ui.label(egui::RichText::new("The real key is displayed here as a single-line value.").size(12.0).color(TEXT_FAINT));
                                    ui.add_space(16.0);
                                    setup_callout(ui, "Keep this key private", "Anyone with this key can control your Nostr identity.", true);
                                    ui.add_space(20.0);
                                    if setup_primary_button(ui, "I have saved my recovery key") { next = Some(SetupStep::Pairing); }
                                }
                                SetupStep::Pairing => {
                                    ui.label(egui::RichText::new("Connect your phone (optional)").size(24.0).strong());
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new("Use NIP-46 approval when unlocking this encrypted home. Both devices need access to the same relay.").size(14.0).color(muted));
                                    ui.add_space(20.0);
                                    setup_field(ui, "Relay", &mut setup.relay, "wss://relay.damus.io", false, Some("Use the same relay on this computer and your phone."));
                                    ui.add_space(16.0);
                                    if let Some(qr) = &setup.pair_qr {
                                        ui.add(egui::Image::new(qr).fit_to_exact_size(egui::vec2(200.0, 200.0)));
                                        ui.add_space(12.0);
                                    }
                                    if let Some(uri) = &setup.pair_uri {
                                        ui.label(egui::RichText::new("Waiting for phone approval\u{2026}").size(16.0).strong());
                                        if setup.pair_qr.is_none() {
                                            ui.label(egui::RichText::new("QR rendering is unavailable; select and copy the connection URI below.").color(muted));
                                        }
                                        ui.add_space(10.0);
                                        setup_callout(ui, "Your identity key is not shared", "The phone signer approves the connection request.", false);
                                        ui.add_space(12.0);
                                        egui::Frame::new()
                                            .fill(RAISED)
                                            .stroke(egui::Stroke::new(1.0, LINE))
                                            .corner_radius(10.0)
                                            .inner_margin(egui::Margin::symmetric(16, 12))
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.label(egui::RichText::new("Can't scan? Select and copy the connection URI.").strong().size(13.0));
                                                ui.add_space(6.0);
                                                ui.add(egui::Label::new(egui::RichText::new(uri).color(SETUP_ACCENT).size(12.0)).selectable(true));
                                            });
                                        ui.add_space(8.0);
                                    } else if setup.pair_rx.is_some() {
                                        ui.label(egui::RichText::new("Connecting to the relay\u{2026}").color(muted));
                                        ui.add_space(8.0);
                                    }
                                    if setup.pairing_ready {
                                        ui.colored_label(SUCCESS, "Phone paired. The home passphrase remains available as fallback.");
                                        ui.label(egui::RichText::new("If homed asks for a token PIN during phone unlock, enter any non-empty value, then approve the request on your phone.").color(muted));
                                        if let Some(pubkey) = &setup.paired_user_pubkey {
                                            ui.add_space(6.0);
                                            ui.label("Phone signer identity:");
                                            ui.add(egui::Label::new(pubkey).selectable(true));
                                        }
                                        ui.add_space(8.0);
                                    }
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        if !setup.pairing_ready && setup.pair_rx.is_none() && setup_primary_button(ui, "Pair phone") {
                                            if let Err(e) = setup.start_pairing(ui.ctx().clone()) { setup.error = Some(e); }
                                        }
                                        if setup.pairing_ready && setup_primary_button(ui, "Continue with phone pairing") {
                                            next = Some(SetupStep::Passphrase);
                                        }
                                        ui.add_space(12.0);
                                        if setup_secondary_button(ui, "Skip for now") {
                                            if let Some(cancel) = setup.pair_cancel.take() { let _ = cancel.send(()); }
                                            setup.pair_rx = None;
                                            if let Some(uri) = &mut setup.pair_uri { uri.zeroize(); }
                                            setup.pair_uri = None;
                                            setup.pair_qr = None;
                                            setup.pairing_ready = false;
                                            next = Some(SetupStep::Passphrase);
                                        }
                                        ui.add_space(12.0);
                                        ui.label(egui::RichText::new("Skip pairing and continue to set your passphrase.").size(12.0).color(TEXT_FAINT));
                                    });
                                }
                                SetupStep::Passphrase => {
                                    ui.label(egui::RichText::new("Choose your home passphrase").size(24.0).strong());
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new("This passphrase unlocks your encrypted home, including when you are offline.").size(14.0).color(muted));
                                    ui.add_space(20.0);
                                    setup_field(ui, "Passphrase", &mut setup.passphrase, "At least 8 characters", true, Some("Use at least 8 characters."));
                                    ui.add_space(16.0);
                                    setup_field(ui, "Repeat passphrase", &mut setup.confirmation, "Enter the same passphrase again", true, None);
                                    ui.add_space(20.0);
                                    ui.horizontal(|ui| {
                                        if setup_primary_button(ui, "Create account") {
                                            if setup.passphrase.len() < 8 { setup.error = Some("Use at least 8 characters.".into()); }
                                            else if setup.passphrase != setup.confirmation { setup.error = Some("The passphrases do not match.".into()); }
                                            else { setup.error = None; create = true; }
                                        }
                                        ui.add_space(16.0);
                                        ui.label(egui::RichText::new("Evermore creates the encrypted home after you continue.").size(13.0).color(TEXT_FAINT));
                                    });
                                }
                                SetupStep::Saving => {
                                    ui.label(egui::RichText::new("Creating your encrypted home\u{2026}").size(24.0).strong());
                                    ui.add_space(10.0);
                                    ui.add(egui::Spinner::new());
                                }
                            }
                            if let Some(error) = &setup.error { ui.add_space(12.0); ui.colored_label(ui.visuals().error_fg_color, error); }
                        });
                    });
                });
        });
        if create {
            setup.step = SetupStep::Saving;
            match setup.create() {
                Ok(()) => std::process::exit(0),
                Err(error) => {
                    setup.step = SetupStep::Passphrase;
                    setup.error = Some(error);
                }
            }
        }
        let pairing_lines = setup
            .pair_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for mut line in pairing_lines {
            if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&line) {
                match value.get("state").and_then(|s| s.as_str()) {
                    Some("waiting") => {
                        if let Some(uri_value) = value.get_mut("uri") {
                            if let serde_json::Value::String(uri) =
                                std::mem::replace(uri_value, serde_json::Value::Null)
                            {
                                if let Some(previous) = &mut setup.pair_uri {
                                    previous.zeroize();
                                }
                                setup.pair_uri = Some(uri);
                                if let Some(uri) = setup.pair_uri.as_deref() {
                                    setup.pair_qr = make_qr_texture(ui.ctx(), uri);
                                }
                            }
                        }
                    }
                    Some("paired") => {
                        setup.pairing_ready = true;
                        setup.paired_user_pubkey = value
                            .get("user_pubkey")
                            .and_then(|s| s.as_str())
                            .map(str::to_owned);
                        setup.pair_rx = None;
                        setup.pair_cancel = None;
                        if let Some(uri) = &mut setup.pair_uri {
                            uri.zeroize();
                        }
                        setup.pair_uri = None;
                        setup.pair_qr = None;
                    }
                    Some("error") => {
                        setup.error = value
                            .get("message")
                            .and_then(|s| s.as_str())
                            .map(str::to_owned);
                        setup.pair_rx = None;
                        setup.pair_cancel = None;
                        if let Some(uri) = &mut setup.pair_uri {
                            uri.zeroize();
                        }
                        setup.pair_uri = None;
                        setup.pair_qr = None;
                    }
                    _ => {}
                }
            }
            line.zeroize();
        }
        if let Some(step) = next {
            setup.step = step;
        }
        self.screen = Screen::FirstUser(setup);
    }

    fn ui_picker(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, error: Option<String>) {
        let mut selected = None;
        let user_names: Vec<String> = self.users.iter().map(|u| u.user_name.clone()).collect();

        // Keyboard quick-select (1-9) alongside clicking -- genuinely
        // useful on a login screen, not just a test hook, and it's what
        // makes this screen operable without a pointing device at all.
        const DIGIT_KEYS: [egui::Key; 9] = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
            egui::Key::Num8,
            egui::Key::Num9,
        ];
        ui.input(|i| {
            for (index, key) in DIGIT_KEYS.iter().enumerate() {
                if i.key_pressed(*key)
                    && let Some(user_name) = user_names.get(index)
                {
                    selected = Some(user_name.clone());
                }
            }
        });

        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.heading("Welcome");
            if user_names.is_empty() {
                ui.add_space(24.0);
                ui.heading("No identities are enrolled");
                ui.add_space(8.0);
                ui.label("First-user setup did not finish on this system.");
                ui.add_space(12.0);
                ui.weak(
                    "Switch to a text terminal, resolve the enrollment error, then restart greetd.",
                );
            }
            if let Some(error) = &error {
                ui.add_space(8.0);
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.add_space(32.0);
            // `vertical_centered` doesn't actually center a nested
            // `horizontal` row in this egui version -- confirmed
            // visually on the VM, not a hypothetical -- so the row's
            // left padding is computed by hand instead: avatar tiles
            // are a fixed 96px plus 24px spacing, so the total content
            // width is exactly known up front.
            const TILE_WIDTH: f32 = 96.0;
            const TILE_SPACING: f32 = 24.0;
            let user_count = user_names.len() as f32;
            let content_width =
                user_count * TILE_WIDTH + (user_count - 1.0).max(0.0) * TILE_SPACING;
            let left_padding = ((ui.available_width() - content_width) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(left_padding);
                for (index, user_name) in user_names.iter().enumerate() {
                    let display_name = self
                        .users
                        .iter()
                        .find(|u| &u.user_name == user_name)
                        .map(|u| u.display_name.clone())
                        .unwrap_or_default();
                    ui.vertical(|ui| {
                        let texture = self.avatar_texture(ctx, user_name);
                        let clicked = if let Some(texture) = texture {
                            let image = egui::Image::new(&texture)
                                .fit_to_exact_size(egui::vec2(96.0, 96.0))
                                .corner_radius(48.0)
                                .sense(egui::Sense::click());
                            ui.add(image).clicked()
                        } else {
                            initial_avatar(ui, &display_name, 96.0).clicked()
                        };
                        ui.label(&display_name);
                        if index < 9 {
                            ui.weak(format!("[{}]", index + 1));
                        }
                        if clicked {
                            selected = Some(user_name.clone());
                        }
                    });
                    if index + 1 < user_names.len() {
                        ui.add_space(TILE_SPACING);
                    }
                }
            });
        });
        self.screen = Screen::Picker { error };
        if let Some(user_name) = selected {
            self.select_user(&user_name);
        }
    }

    fn ui_auth(
        &mut self,
        ui: &mut egui::Ui,
        user_name: String,
        prompt: String,
        secret: bool,
        mut input: String,
        mut focus_requested: bool,
    ) {
        let mut submit = false;
        let mut cancel = false;
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.heading(&user_name);
            ui.add_space(20.0);
            ui.label(&prompt);
            let response = ui.add(
                egui::TextEdit::singleline(&mut input)
                    .password(secret)
                    .desired_width(240.0),
            );
            // Request focus exactly once per prompt (tracked via
            // `focus_requested`), not reactively every frame based on
            // "is anything focused right now": that check runs *after*
            // the text edit's own Enter-handling may have already
            // surrendered focus this same frame, so a reactive check
            // immediately re-grabs it right back -- silently defeating
            // Enter-to-submit. Found live-testing on the VM, not
            // hypothetical.
            if !focus_requested {
                response.request_focus();
                focus_requested = true;
            }
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            ui.add_space(8.0);
            let pad = centered_row_padding(ui, &["Log in", "Cancel"]);
            ui.horizontal(|ui| {
                ui.add_space(pad);
                if ui.button("Log in").clicked() {
                    submit = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });

        if cancel {
            if let Some(mut client) = self.client.take() {
                client.cancel_session();
            }
            self.screen = Screen::Picker { error: None };
        } else if submit {
            self.screen = Screen::Auth {
                user_name: user_name.clone(),
                prompt,
                secret,
                input: input.clone(),
                focus_requested,
            };
            self.submit(&user_name, Some(&input));
        } else {
            self.screen = Screen::Auth {
                user_name,
                prompt,
                secret,
                input,
                focus_requested,
            };
        }
    }
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "evermore-greeter",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            load_fonts(&cc.egui_ctx);
            apply_theme(&cc.egui_ctx);
            let background = load_background_texture(&cc.egui_ctx);
            Ok(Box::new(GreeterApp::new(background)))
        }),
    )
}
