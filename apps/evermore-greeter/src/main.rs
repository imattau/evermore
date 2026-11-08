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
use std::collections::HashMap;
use std::path::PathBuf;

/// What a stock greetd frontend would also need to decide: which
/// command actually starts the real desktop session once auth
/// succeeds. Evermore's own answer -- Sway, with whatever the user's
/// own `~/.config/sway/config` includes (see
/// `packaging/sway/install-desktop.sh`) -- since that's this project's
/// one supported desktop, not a configurable session picker.
const SESSION_CMD: &[&str] = &["/usr/bin/sway"];

struct UserEntry {
    user_name: String,
    display_name: String,
    avatar_path: Option<PathBuf>,
}

enum Screen {
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

struct GreeterApp {
    users: Vec<UserEntry>,
    avatars: HashMap<String, Option<TextureHandle>>,
    screen: Screen,
    client: Option<GreetdClient>,
}

impl GreeterApp {
    fn new() -> Self {
        match homed::list_users() {
            Ok(entries) => {
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
                    screen: Screen::Picker { error: None },
                    client: None,
                }
            }
            Err(e) => Self {
                users: Vec::new(),
                avatars: HashMap::new(),
                screen: Screen::FatalError(format!("listing homed users failed: {e}")),
                client: None,
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
        egui::CentralPanel::default().show(ui, |ui| {
            match std::mem::replace(&mut self.screen, Screen::Starting) {
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
                        ui.colored_label(Color32::from_rgb(220, 80, 80), "evermore-greeter error");
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
                ui.colored_label(Color32::from_rgb(220, 80, 80), error);
            }
            ui.add_space(30.0);
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
            ui.add_space(10.0);
            ui.horizontal(|ui| {
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
        Box::new(|_cc| Ok(Box::new(GreeterApp::new()))),
    )
}
