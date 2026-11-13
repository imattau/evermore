use eframe::egui;
use freedesktop_desktop_entry::{
    DesktopEntry, current_desktop, default_paths, get_languages_from_env,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const APP_ID: &str = "org.evermore.Launcher";
const APP_TITLE: &str = "Evermore Launcher";
const WINDOW_SIZE: [f32; 2] = [900.0, 620.0];
const TILE_SIZE: egui::Vec2 = egui::vec2(148.0, 128.0);
const ICON_SIZE: f32 = 52.0;

#[derive(Clone)]
struct AppEntry {
    id: String,
    name: String,
    generic_name: String,
    comment: String,
    keywords: Vec<String>,
    icon_path: Option<PathBuf>,
    desktop: DesktopEntry,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FavoritesFile {
    version: u32,
    #[serde(default)]
    app_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    #[serde(default)]
    apps: HashMap<String, Usage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Usage {
    launches: u64,
    last_launched: u64,
}

#[derive(Clone, Copy)]
struct TilePalette {
    card: egui::Color32,
    line: egui::Color32,
    text: egui::Color32,
    accent: egui::Color32,
}

#[derive(Default)]
struct PendingActions {
    launch: Option<String>,
    favorite: Option<String>,
}

#[derive(Default)]
struct Launcher {
    apps: Vec<AppEntry>,
    favorites: FavoritesFile,
    history: HistoryFile,
    favorites_path: PathBuf,
    history_path: PathBuf,
    query: String,
    previous_query: String,
    selected: Option<String>,
    search_focused: bool,
    error: Option<String>,
    focus_was_seen: bool,
}

fn main() -> eframe::Result {
    let apps = load_apps();
    let (favorites_path, history_path) = state_paths();
    let favorites = load_json::<FavoritesFile>(&favorites_path);
    let history = load_json::<HistoryFile>(&history_path);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id(APP_ID)
            .with_title(APP_TITLE)
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size([560.0, 420.0])
            .with_decorations(false)
            .with_resizable(true),
        ..Default::default()
    };

    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            load_fonts(&cc.egui_ctx);
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(Launcher {
                apps,
                favorites,
                history,
                favorites_path,
                history_path,
                search_focused: true,
                ..Default::default()
            }))
        }),
    )
}

fn load_apps() -> Vec<AppEntry> {
    let locales = get_languages_from_env();
    let desktops = current_desktop().unwrap_or_default();
    let paths: Vec<_> = default_paths().collect();
    load_apps_from_paths(&paths, &locales, &desktops)
}

fn load_apps_from_paths(
    paths: &[PathBuf],
    locales: &[String],
    desktops: &[String],
) -> Vec<AppEntry> {
    let mut seen_ids = HashSet::new();
    let mut apps = Vec::new();

    // default_paths is ordered by XDG precedence. Mark an ID as seen before
    // filtering so a user-level Hidden entry masks a system-level app.
    for applications_dir in paths {
        collect_desktop_files(applications_dir, &mut |path| {
            let Some(id) = desktop_file_id(applications_dir, path) else {
                return;
            };
            if !seen_ids.insert(id.clone()) {
                return;
            }
            let Ok(desktop) = DesktopEntry::from_path(path, Some(locales)) else {
                return;
            };
            if !is_visible_application(&desktop, desktops) {
                return;
            }
            let name = desktop
                .name(locales)
                .map_or_else(|| id.clone(), |s| s.into_owned());
            let generic_name = desktop
                .generic_name(locales)
                .map_or_else(String::new, |s| s.into_owned());
            let comment = desktop
                .comment(locales)
                .map_or_else(String::new, |s| s.into_owned());
            let keywords = desktop
                .keywords(locales)
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.into_owned())
                .collect();
            let icon_path = desktop.icon().and_then(resolve_icon);
            apps.push(AppEntry {
                id,
                name,
                generic_name,
                comment,
                keywords,
                icon_path,
                desktop,
            });
        });
    }
    apps.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    apps
}

fn collect_desktop_files(directory: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_files(&path, visit);
        } else if path.extension().is_some_and(|ext| ext == "desktop") {
            visit(&path);
        }
    }
}

fn desktop_file_id(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?.to_str()?;
    Some(relative.strip_suffix(".desktop")?.replace(['/', '\\'], "-"))
}

fn is_visible_application(entry: &DesktopEntry, desktops: &[String]) -> bool {
    if entry.hidden() || entry.no_display() || entry.type_() != Some("Application") {
        return false;
    }
    if entry.only_show_in().is_some_and(|only| {
        !desktops
            .iter()
            .any(|desktop| only.contains(&desktop.as_str()))
    }) {
        return false;
    }
    if entry.not_show_in().is_some_and(|blocked| {
        desktops
            .iter()
            .any(|desktop| blocked.contains(&desktop.as_str()))
    }) {
        return false;
    }
    if entry
        .try_exec()
        .is_some_and(|try_exec| !executable_exists(try_exec))
    {
        return false;
    }
    entry.dbus_activatable() || entry.exec().is_some()
}

fn executable_exists(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return is_executable(path);
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .any(|directory| is_executable(&directory.join(program)))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn resolve_icon(icon: &str) -> Option<PathBuf> {
    let path = PathBuf::from(icon);
    if path.is_absolute() && path.is_file() {
        return Some(path);
    }
    let theme = freedesktop_icons::default_theme_gtk();
    let mut builder = freedesktop_icons::lookup(icon).with_size(64).with_cache();
    if let Some(theme) = &theme {
        builder = builder.with_theme(theme);
    }
    builder.find()
}

fn state_paths() -> (PathBuf, PathBuf) {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"));
    (
        config.join("evermore/launcher.json"),
        state.join("evermore/launcher-history.json"),
    )
}

fn load_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    let Ok(contents) = fs::read(path) else {
        return T::default();
    };
    match serde_json::from_slice(&contents) {
        Ok(value) => value,
        Err(error) => {
            eprintln!(
                "evermore-launcher: ignoring invalid state {}: {error}",
                path.display()
            );
            let damaged = path.with_extension(format!("corrupt-{}", now_seconds()));
            let _ = fs::rename(path, damaged);
            T::default()
        }
    }
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "state path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let temporary = path.with_extension(format!("tmp-{}-{}", std::process::id(), now_nanos()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("save {}: {error}", path.display()))
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn launch(entry: &AppEntry) -> Result<(), String> {
    if entry.desktop.dbus_activatable() {
        use zbus::blocking::{Connection, Proxy};
        use zbus::zvariant::OwnedValue;
        let connection =
            Connection::session().map_err(|error| format!("connect to session bus: {error}"))?;
        let proxy = Proxy::new(
            &connection,
            entry.desktop.appid.as_str(),
            "/org/freedesktop/Application",
            "org.freedesktop.Application",
        )
        .map_err(|error| format!("prepare D-Bus activation for {}: {error}", entry.name))?;
        let platform_data: HashMap<String, OwnedValue> = HashMap::new();
        let _: () = proxy
            .call("Activate", &(platform_data,))
            .map_err(|error| format!("activate {} over D-Bus: {error}", entry.name))?;
        return Ok(());
    }

    let args = entry
        .desktop
        .parse_exec()
        .map_err(|error| format!("invalid Exec entry for {}: {error}", entry.name))?;
    let Some(program) = args.first() else {
        return Err(format!("{} has no launch command", entry.name));
    };
    let terminal = entry.desktop.terminal();
    let mut command = if terminal {
        let mut command = Command::new("foot");
        command.arg("-e").arg(program).args(&args[1..]);
        command
    } else {
        let mut command = Command::new(program);
        command.args(&args[1..]);
        command
    };
    if let Some(directory) = entry.desktop.path() {
        command.current_dir(directory);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not start {}: {error}", entry.name))
}

impl eframe::App for Launcher {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root_ui.ctx().clone();
        let focused = ctx.input(|input| input.focused);
        if focused {
            self.focus_was_seen = true;
        } else if self.focus_was_seen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let mut pending = PendingActions::default();
        let dark = root_ui.visuals().dark_mode;
        let (background, card, line, text, muted, accent) = palette(dark);
        egui::Frame::new()
            .fill(background)
            .inner_margin(egui::Margin::same(22))
            .show(root_ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Apps");
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("{} installed", self.apps.len())).color(muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Close launcher (Esc)").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
                ui.add_space(10.0);
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search apps")
                        .font(egui::FontId::proportional(16.0))
                        .desired_width(f32::INFINITY)
                        .min_size(egui::vec2(0.0, 48.0)),
                );
                if self.search_focused {
                    search.request_focus();
                    self.search_focused = false;
                }
                if self.previous_query != self.query {
                    self.previous_query.clone_from(&self.query);
                    self.selected = None;
                }

                let visible = self.visible_indices();
                if self
                    .selected
                    .as_ref()
                    .is_none_or(|id| !visible.iter().any(|index| self.apps[*index].id == *id))
                {
                    self.selected = visible.first().map(|index| self.apps[*index].id.clone());
                }
                if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if !visible.is_empty() {
                    ctx.input(|input| {
                        let selected_index = self
                            .selected
                            .as_ref()
                            .and_then(|id| {
                                visible.iter().position(|index| self.apps[*index].id == *id)
                            })
                            .unwrap_or(0);
                        if input.key_pressed(egui::Key::ArrowDown)
                            || input.key_pressed(egui::Key::ArrowRight)
                        {
                            self.selected = Some(
                                self.apps[visible[(selected_index + 1) % visible.len()]]
                                    .id
                                    .clone(),
                            );
                        } else if input.key_pressed(egui::Key::ArrowUp)
                            || input.key_pressed(egui::Key::ArrowLeft)
                        {
                            let previous = if selected_index == 0 {
                                visible.len() - 1
                            } else {
                                selected_index - 1
                            };
                            self.selected = Some(self.apps[visible[previous]].id.clone());
                        }
                    });
                    if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
                        pending.launch = self.selected.clone();
                    }
                    let favorite_shortcut =
                        egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::D);
                    if ctx.input_mut(|input| input.consume_shortcut(&favorite_shortcut)) {
                        pending.favorite = self.selected.clone();
                    }
                }

                ui.add_space(12.0);
                if !self.query.trim().is_empty() {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} result{}",
                            visible.len(),
                            if visible.len() == 1 { "" } else { "s" }
                        ))
                        .color(muted),
                    );
                    if visible.is_empty() {
                        ui.add_space(36.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "No apps match ‘{}’",
                                    self.query.trim()
                                ))
                                .size(17.0),
                            );
                            ui.label(
                                egui::RichText::new("Try another app name or keyword.")
                                    .color(muted),
                            );
                        });
                    } else {
                        self.draw_grid(
                            ui,
                            "search-results",
                            &visible,
                            TilePalette {
                                card,
                                line,
                                text,
                                accent,
                            },
                            &mut pending,
                        );
                    }
                } else {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("ALL APPS")
                                .small()
                                .strong()
                                .color(muted),
                        );
                        ui.label(
                            egui::RichText::new("·  Star an app or press Ctrl+D to pin it")
                                .small()
                                .color(muted),
                        );
                    });
                    self.draw_grid(
                        ui,
                        "all-apps",
                        &visible,
                        TilePalette {
                            card,
                            line,
                            text,
                            accent,
                        },
                        &mut pending,
                    );
                }
                if let Some(error) = &self.error {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(error).color(egui::Color32::from_rgb(224, 108, 117)),
                    );
                }
            });

        if let Some(id) = pending.favorite {
            self.toggle_favorite(&id);
            self.search_focused = true;
        }
        if let Some(id) = pending.launch
            && let Some(entry) = self.apps.iter().find(|entry| entry.id == id)
        {
            match launch(entry) {
                Ok(()) => {
                    self.record_launch(&id);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
}

impl Launcher {
    fn visible_indices(&self) -> Vec<usize> {
        let query = self.query.trim();
        let mut scored: Vec<(i64, usize)> = self
            .apps
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let score = if query.is_empty() {
                    0
                } else {
                    app_score(entry, query)?
                };
                Some((score, index))
            })
            .collect();
        scored.sort_by(|(score_a, index_a), (score_b, index_b)| {
            let app_a = &self.apps[*index_a];
            let app_b = &self.apps[*index_b];
            score_b
                .cmp(score_a)
                .then_with(|| {
                    self.favorites
                        .app_ids
                        .contains(&app_b.id)
                        .cmp(&self.favorites.app_ids.contains(&app_a.id))
                })
                .then_with(|| {
                    self.usage_for(&app_b.id)
                        .last_launched
                        .cmp(&self.usage_for(&app_a.id).last_launched)
                })
                .then_with(|| app_a.name.to_lowercase().cmp(&app_b.name.to_lowercase()))
        });
        scored.into_iter().map(|(_, index)| index).collect()
    }

    fn usage_for(&self, id: &str) -> Usage {
        self.history.apps.get(id).cloned().unwrap_or_default()
    }

    fn toggle_favorite(&mut self, id: &str) {
        if !self.favorites.app_ids.remove(id) {
            self.favorites.app_ids.insert(id.to_owned());
        }
        self.favorites.version = 1;
        if let Err(error) = save_json(&self.favorites_path, &self.favorites) {
            self.error = Some(error);
        } else {
            self.error = None;
        }
    }

    fn record_launch(&mut self, id: &str) {
        let usage = self.history.apps.entry(id.to_owned()).or_default();
        usage.launches = usage.launches.saturating_add(1);
        usage.last_launched = now_millis();
        self.history.version = 1;
        if let Err(error) = save_json(&self.history_path, &self.history) {
            eprintln!("evermore-launcher: {error}");
        }
    }

    fn draw_grid(
        &mut self,
        ui: &mut egui::Ui,
        grid_id: &str,
        indices: &[usize],
        colors: TilePalette,
        pending: &mut PendingActions,
    ) {
        egui::ScrollArea::vertical()
            .id_salt(("launcher-scroll", grid_id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let available = ui.available_width();
                let columns = ((available + 10.0) / (TILE_SIZE.x + 10.0)).floor().max(1.0) as usize;
                let cell_width = ((available - (columns.saturating_sub(1) as f32 * 10.0))
                    / columns as f32)
                    .max(110.0);
                egui::Grid::new(("launcher-app-grid", grid_id))
                    .num_columns(columns)
                    .spacing([10.0, 10.0])
                    .show(ui, |ui| {
                        for (position, index) in indices.iter().enumerate() {
                            let entry = &self.apps[*index];
                            let id = entry.id.clone();
                            let selected = self.selected.as_deref() == Some(&id);
                            let favorite = self.favorites.app_ids.contains(&id);
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(cell_width, TILE_SIZE.y),
                                egui::Sense::click(),
                            );
                            let fill = if selected {
                                colors.accent.gamma_multiply(0.22)
                            } else if response.hovered() {
                                colors.card.gamma_multiply(1.4)
                            } else {
                                colors.card
                            };
                            let stroke = if selected {
                                egui::Stroke::new(1.5, colors.accent)
                            } else {
                                egui::Stroke::new(1.0, colors.line)
                            };
                            ui.painter()
                                .rect(rect, 12.0, fill, stroke, egui::StrokeKind::Inside);

                            let icon_rect = egui::Rect::from_center_size(
                                egui::pos2(rect.center().x, rect.top() + 40.0),
                                egui::vec2(ICON_SIZE, ICON_SIZE),
                            );
                            if let Some(icon) = &entry.icon_path {
                                if let Ok(uri) = url::Url::from_file_path(icon) {
                                    egui::Image::from_uri(uri.to_string())
                                        .fit_to_exact_size(icon_rect.size())
                                        .paint_at(ui, icon_rect);
                                } else {
                                    paint_fallback_icon(ui, icon_rect, &entry.name, colors.accent);
                                }
                            } else {
                                paint_fallback_icon(ui, icon_rect, &entry.name, colors.accent);
                            }

                            let mut title_job = egui::text::LayoutJob::simple(
                                entry.name.clone(),
                                egui::FontId::proportional(13.0),
                                colors.text,
                                cell_width - 18.0,
                            );
                            title_job.halign = egui::Align::Center;
                            title_job.wrap.max_rows = 2;
                            let title_galley = ui.painter().layout_job(title_job);
                            let title_area = egui::Rect::from_min_max(
                                egui::pos2(rect.left() + 9.0, icon_rect.bottom() + 8.0),
                                egui::pos2(rect.right() - 9.0, rect.bottom() - 10.0),
                            );
                            let title_pos = egui::pos2(
                                title_area.center().x,
                                title_area.center().y - title_galley.size().y / 2.0,
                            );
                            ui.painter().galley(title_pos, title_galley, colors.text);

                            let star_rect = egui::Rect::from_min_size(
                                egui::pos2(rect.right() - 28.0, rect.top() + 7.0),
                                egui::vec2(22.0, 22.0),
                            );
                            let star = ui.interact(
                                star_rect,
                                ui.make_persistent_id(("favorite", &id)),
                                egui::Sense::click(),
                            );
                            ui.painter().text(
                                star_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                if favorite { "★" } else { "☆" },
                                egui::FontId::proportional(18.0),
                                if favorite || star.hovered() {
                                    colors.accent
                                } else {
                                    colors.line
                                },
                            );
                            if star.clicked() {
                                pending.favorite = Some(id.clone());
                            } else if response.clicked() {
                                pending.launch = Some(id.clone());
                            } else if response.hovered() {
                                self.selected = Some(id.clone());
                            }
                            if !entry.generic_name.is_empty() || !entry.comment.is_empty() {
                                let description = if entry.generic_name.is_empty() {
                                    entry.comment.as_str()
                                } else {
                                    entry.generic_name.as_str()
                                };
                                response.clone().on_hover_text(description);
                            }
                            if position % columns == columns - 1 {
                                ui.end_row();
                            }
                        }
                        if !indices.len().is_multiple_of(columns) {
                            ui.end_row();
                        }
                    });
            });
    }
}

fn app_score(entry: &AppEntry, query: &str) -> Option<i64> {
    let fields = std::iter::once(entry.name.as_str())
        .chain(std::iter::once(entry.generic_name.as_str()))
        .chain(std::iter::once(entry.comment.as_str()))
        .chain(std::iter::once(entry.id.as_str()))
        .chain(entry.keywords.iter().map(String::as_str));
    let fields: Vec<_> = fields.filter(|field| !field.is_empty()).collect();
    let mut total = 0;
    for token in query.split_whitespace() {
        let best = fields
            .iter()
            .enumerate()
            .filter_map(|(index, field)| {
                subsequence_score(field, token).map(|score| score + if index == 0 { 30 } else { 0 })
            })
            .max()?;
        total += best;
    }
    Some(total)
}

fn subsequence_score(haystack: &str, needle: &str) -> Option<i64> {
    let haystack: Vec<char> = haystack.to_lowercase().chars().collect();
    let needle: Vec<char> = needle.to_lowercase().chars().collect();
    if needle.is_empty() {
        return Some(0);
    }
    let needle_len = needle.len();
    let mut cursor = 0;
    let mut score = 0_i64;
    let mut previous = None;
    for wanted in needle {
        let found = (cursor..haystack.len()).find(|index| haystack[*index] == wanted)?;
        let consecutive = previous.is_some_and(|index: usize| found == index + 1);
        let boundary = found == 0 || !haystack[found - 1].is_alphanumeric();
        score += if consecutive { 12 } else { 0 };
        score += if boundary { 10 } else { 0 };
        score -= found.saturating_sub(cursor) as i64;
        previous = Some(found);
        cursor = found + 1;
    }
    score += 20 - (haystack.len().saturating_sub(needle_len) as i64).min(20);
    Some(score)
}

fn paint_fallback_icon(ui: &egui::Ui, rect: egui::Rect, name: &str, accent: egui::Color32) {
    let fill = accent.gamma_multiply(0.18);
    ui.painter().rect_filled(rect, 12.0, fill);
    let letter = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional(28.0),
        accent,
    );
}

fn palette(
    dark: bool,
) -> (
    egui::Color32,
    egui::Color32,
    egui::Color32,
    egui::Color32,
    egui::Color32,
    egui::Color32,
) {
    if dark {
        (
            egui::Color32::from_rgb(20, 23, 29),
            egui::Color32::from_rgb(26, 30, 37),
            egui::Color32::from_rgb(48, 55, 67),
            egui::Color32::from_rgb(230, 233, 239),
            egui::Color32::from_rgb(167, 176, 191),
            egui::Color32::from_rgb(110, 168, 254),
        )
    } else {
        (
            egui::Color32::from_rgb(255, 255, 255),
            egui::Color32::from_rgb(241, 243, 245),
            egui::Color32::from_rgb(210, 214, 219),
            egui::Color32::from_rgb(20, 23, 29),
            egui::Color32::from_rgb(90, 98, 112),
            egui::Color32::from_rgb(60, 126, 214),
        )
    }
}

fn load_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        (
            "inter-regular",
            include_bytes!("../../evermore-panel/assets/fonts/Inter-Regular.ttf").as_slice(),
        ),
        (
            "inter-medium",
            include_bytes!("../../evermore-panel/assets/fonts/Inter-Medium.ttf").as_slice(),
        ),
        (
            "inter-semibold",
            include_bytes!("../../evermore-panel/assets/fonts/Inter-SemiBold.ttf").as_slice(),
        ),
        (
            "jetbrains-mono",
            include_bytes!("../../evermore-panel/assets/fonts/JetBrainsMono-Regular.ttf")
                .as_slice(),
        ),
    ] {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
    }
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "inter-regular".into());
    fonts.families.insert(
        egui::FontFamily::Name("inter-medium".into()),
        vec!["inter-medium".into()],
    );
    fonts.families.insert(
        egui::FontFamily::Name("inter-semibold".into()),
        vec!["inter-semibold".into()],
    );
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains-mono".into());
    ctx.set_fonts(fonts);
}

fn apply_theme(ctx: &egui::Context) {
    let (background, card, line, text, muted, accent) = palette(true);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = background;
    visuals.window_fill = background;
    visuals.extreme_bg_color = egui::Color32::from_rgb(33, 38, 47);
    visuals.faint_bg_color = card;
    visuals.override_text_color = Some(text);
    visuals.weak_text_color = Some(muted);
    visuals.selection.bg_fill = accent;
    visuals.selection.stroke = egui::Stroke::new(1.0, accent);
    visuals.widgets.inactive.bg_fill = card;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, line);
    visuals.widgets.hovered.bg_fill = accent.gamma_multiply(0.32);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    visuals.window_corner_radius = egui::CornerRadius::same(16);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(12);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(12);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(12);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(20.0));
    ctx.set_style_of(egui::Theme::Dark, style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "evermore-launcher-test-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn entry(id: &str, text: &str) -> AppEntry {
        let path = PathBuf::from(format!("{id}.desktop"));
        let desktop = DesktopEntry::from_str(path, text, None::<&[String]>).unwrap();
        AppEntry {
            id: id.into(),
            name: desktop
                .name(&[] as &[String])
                .map_or_else(|| id.into(), |s| s.into_owned()),
            generic_name: desktop
                .generic_name(&[] as &[String])
                .map_or_else(String::new, |s| s.into_owned()),
            comment: desktop
                .comment(&[] as &[String])
                .map_or_else(String::new, |s| s.into_owned()),
            keywords: desktop
                .keywords(&[] as &[String])
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.into_owned())
                .collect(),
            icon_path: None,
            desktop,
        }
    }

    #[test]
    fn desktop_ids_flatten_nested_paths() {
        let root = Path::new("/apps");
        assert_eq!(
            desktop_file_id(root, Path::new("/apps/vendor/editor.desktop")).as_deref(),
            Some("vendor-editor")
        );
    }

    #[test]
    fn hidden_and_desktop_visibility_rules_are_applied() {
        let hidden = entry(
            "hidden",
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nHidden=true\n",
        );
        assert!(!is_visible_application(&hidden.desktop, &[]));
        let nodisplay = entry(
            "private",
            "[Desktop Entry]\nType=Application\nName=Private\nExec=private\nNoDisplay=true\n",
        );
        assert!(!is_visible_application(&nodisplay.desktop, &[]));
        let only = entry(
            "only",
            "[Desktop Entry]\nType=Application\nName=Only\nExec=only\nOnlyShowIn=Sway;\n",
        );
        assert!(is_visible_application(&only.desktop, &["Sway".into()]));
        assert!(!is_visible_application(&only.desktop, &["GNOME".into()]));
        let not_show = entry(
            "blocked",
            "[Desktop Entry]\nType=Application\nName=Blocked\nExec=blocked\nNotShowIn=Sway;\n",
        );
        assert!(!is_visible_application(&not_show.desktop, &["Sway".into()]));
        let missing = entry(
            "missing",
            "[Desktop Entry]\nType=Application\nName=Missing\nExec=missing\nTryExec=/not/a/real/executable\n",
        );
        assert!(!is_visible_application(&missing.desktop, &[]));
    }

    #[test]
    fn xdg_precedence_and_hidden_overrides_are_respected() {
        let dir = temp_dir();
        let user = dir.join("user/applications");
        let system = dir.join("system/applications");
        fs::create_dir_all(&user).unwrap();
        fs::create_dir_all(&system).unwrap();
        let user_app = user.join("same.desktop");
        let system_app = system.join("same.desktop");
        fs::write(
            &user_app,
            "[Desktop Entry]\nType=Application\nName=User Version\nExec=app\n",
        )
        .unwrap();
        fs::write(
            &system_app,
            "[Desktop Entry]\nType=Application\nName=System Version\nExec=app\n",
        )
        .unwrap();
        let apps = load_apps_from_paths(&[user.clone(), system.clone()], &[], &[]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "User Version");

        fs::write(
            &user_app,
            "[Desktop Entry]\nType=Application\nName=Hidden Override\nExec=app\nHidden=true\n",
        )
        .unwrap();
        let apps = load_apps_from_paths(&[user.clone(), system.clone()], &[], &[]);
        assert!(apps.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn localized_names_and_icon_paths_are_used() {
        let locales = vec!["fr".to_owned()];
        let desktop = DesktopEntry::from_str(
            PathBuf::from("localized.desktop"),
            "[Desktop Entry]\nType=Application\nName=Browser\nName[fr]=Navigateur\nExec=browser\n",
            Some(&locales),
        )
        .unwrap();
        assert_eq!(desktop.name(&locales).unwrap(), "Navigateur");

        let dir = temp_dir();
        let icon = dir.join("icon.png");
        fs::write(&icon, b"fixture").unwrap();
        assert_eq!(resolve_icon(icon.to_str().unwrap()), Some(icon.clone()));
        assert_eq!(
            resolve_icon("evermore-launcher-impossible-missing-icon"),
            None
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn search_matches_fuzzy_metadata_and_ranks_names() {
        let browser = entry(
            "browser.desktop",
            "[Desktop Entry]\nType=Application\nName=Firefox\nGenericName=Web Browser\nComment=Browse the web\nKeywords=internet;web;\nExec=firefox\n",
        );
        let notes = entry(
            "notes.desktop",
            "[Desktop Entry]\nType=Application\nName=Notes\nExec=notes\n",
        );
        assert!(app_score(&browser, "ffox").is_some());
        assert!(app_score(&browser, "internet").is_some());
        assert!(app_score(&browser, "browser").unwrap() > app_score(&browser, "brw").unwrap());
        assert!(app_score(&notes, "firefox").is_none());
    }

    #[test]
    fn favorites_and_history_round_trip_and_bad_state_is_preserved() {
        let dir = temp_dir();
        let fav_path = dir.join("config/favorites.json");
        let history_path = dir.join("state/history.json");
        let mut favorites = FavoritesFile {
            version: 1,
            app_ids: BTreeSet::new(),
        };
        favorites.app_ids.insert("org.example.App".into());
        let history = HistoryFile {
            version: 1,
            apps: HashMap::from([(
                "org.example.App".into(),
                Usage {
                    launches: 3,
                    last_launched: 42,
                },
            )]),
        };
        save_json(&fav_path, &favorites).unwrap();
        save_json(&history_path, &history).unwrap();
        let restored: FavoritesFile = load_json(&fav_path);
        let restored_history: HistoryFile = load_json(&history_path);
        assert_eq!(restored.app_ids, favorites.app_ids);
        assert_eq!(restored_history.apps["org.example.App"].launches, 3);

        let corrupt_path = dir.join("state/bad.json");
        fs::create_dir_all(corrupt_path.parent().unwrap()).unwrap();
        fs::write(&corrupt_path, b"not json").unwrap();
        let _: FavoritesFile = load_json(&corrupt_path);
        assert!(!corrupt_path.exists());
        assert!(
            fs::read_dir(corrupt_path.parent().unwrap())
                .unwrap()
                .any(|item| item
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("corrupt-"))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn exec_parser_keeps_arguments_separate() {
        let app = entry(
            "terminal.desktop",
            "[Desktop Entry]\nType=Application\nName=Example\nExec=tool --message \"hello world\"\n",
        );
        let argv = app.desktop.parse_exec().unwrap();
        assert_eq!(argv, ["tool", "--message", "hello world"]);
    }

    #[test]
    fn favorited_apps_stay_at_start_of_full_list() {
        let dir = temp_dir();
        let launcher = Launcher {
            apps: vec![
                entry(
                    "alpha",
                    "[Desktop Entry]\nType=Application\nName=Alpha\nExec=alpha\n",
                ),
                entry(
                    "beta",
                    "[Desktop Entry]\nType=Application\nName=Beta\nExec=beta\n",
                ),
                entry(
                    "gamma",
                    "[Desktop Entry]\nType=Application\nName=Gamma\nExec=gamma\n",
                ),
            ],
            favorites: FavoritesFile {
                version: 1,
                app_ids: BTreeSet::from(["gamma".to_owned()]),
            },
            favorites_path: dir.join("launcher.json"),
            history_path: dir.join("launcher-history.json"),
            ..Default::default()
        };

        let ordered: Vec<&str> = launcher
            .visible_indices()
            .into_iter()
            .map(|index| launcher.apps[index].id.as_str())
            .collect();
        assert_eq!(ordered, ["gamma", "alpha", "beta"]);
        let _ = fs::remove_dir_all(dir);
    }
}
