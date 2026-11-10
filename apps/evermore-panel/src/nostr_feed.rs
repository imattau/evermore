//! The Nostr feed drawer: a right-edge window (`evermore-panel
//! --nostr-feed`) showing recent notes from the user's configured external
//! relays, plus a compose-and-post flow for replies. It is a separate native
//! window from the AI drawer so the two can sit on opposite screen edges.
//!
//! Handing a note to the AI normally goes through `ai_handoff`. Dragging a
//! note also copies its text to the clipboard, and if the pointer is
//! released over the AI drawer the note is handed over as one-turn context.
//! That drop has to be detected with `sway_windows`: Wayland keeps the
//! pointer grab on the feed surface, so the AI window never receives a drop
//! event.

use eframe::egui;
use evermore_common::ai::Role;

use crate::{
    PanelApp, ai_handoff, ai_session_client, nostr_feed_client, signer_client, sway_windows,
};

/// Tokens from the "Evermore Desktop — UI improvements" Figma file's
/// "Nostr feed drawer" page, reused from the greeter's identical values
/// rather than re-deriving them: the accent-tinted highlight chip used
/// for the attached-note card, and the near-black text that sits on it.
const ACCENT_CHIP: egui::Color32 = egui::Color32::from_rgb(38, 70, 120); // #264678
const ACCENT_INK: egui::Color32 = egui::Color32::from_rgb(7, 9, 12); // #07090C
const LINE: egui::Color32 = egui::Color32::from_rgb(48, 55, 67); // #303743

/// The reply composer's "Post" button, the drawer's one filled
/// call-to-action -- every other button here is the neutral bordered
/// style `build_visuals` now gives every button by default.
fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).strong().color(ACCENT_INK))
            .fill(ui.visuals().hyperlink_color)
            .corner_radius(8.0),
    )
    .clicked()
}

impl PanelApp {
    pub(crate) fn nostr_feed_ui(&mut self, ui: &mut egui::Ui) {
        // The feed is a right-edge drawer. Sway has no right-anchor rule, so
        // move the window flush against the workspace's right edge once it
        // has mapped. Retry briefly; give up if this is not Sway.
        if !self.nostr_feed_positioned {
            if sway_windows::anchor_to_right_edge(std::process::id()).is_ok() {
                self.nostr_feed_positioned = true;
            } else {
                let deadline = *self.nostr_feed_position_deadline.get_or_insert_with(|| {
                    std::time::Instant::now() + std::time::Duration::from_secs(2)
                });
                if std::time::Instant::now() >= deadline {
                    self.nostr_feed_positioned = true;
                } else {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        }
        if !self.nostr_feed_loaded {
            self.nostr_feed_loaded = true;
            let relays = crate::settings_client::external_relay_urls();
            if relays.is_empty() {
                self.nostr_feed_error = Some(
                    "No external relays configured yet (Settings \u{2192} evermore:relays.external)."
                        .into(),
                );
            } else {
                let (receiver, handle) = nostr_feed_client::start(relays);
                self.nostr_feed_receiver = Some(receiver);
                self.nostr_feed_handle = Some(handle);
            }
        }
        if let Some(receiver) = &self.nostr_feed_receiver {
            let mut received_any = false;
            while let Ok(note) = receiver.try_recv() {
                if !self
                    .nostr_feed_notes
                    .iter()
                    .any(|existing| existing.id == note.id)
                {
                    self.nostr_feed_notes.push(note);
                    received_any = true;
                }
            }
            if received_any {
                self.nostr_feed_notes
                    .sort_by_key(|note| std::cmp::Reverse(note.created_at));
                self.nostr_feed_notes.truncate(200);
                ui.ctx().request_repaint();
            }
        }
        if !self.identities_loaded {
            self.refresh_identities();
        }
        if self.nostr_post_identity.is_empty()
            && let Some(identity) = self.identities.first()
        {
            self.nostr_post_identity = identity.name.clone();
        }
        // Keep polling the relay receiver even when the window is idle.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));

        if self.nostr_drag_note.is_some()
            && let Some(position) = ui.ctx().input(|input| input.pointer.latest_pos())
        {
            self.nostr_drag_pos = Some(position);
        }
        // A drag source returns no useful response once the pointer is
        // outside the feed, so watch the release itself and check where it
        // landed against the AI drawer's rectangle.
        if self.nostr_drag_note.is_some() && ui.ctx().input(|input| input.pointer.any_released()) {
            self.finish_note_drag(ui.ctx());
        }

        if self.nostr_feed_collapsed {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(8, 10))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        if ui
                            .button("\u{2039}")
                            .on_hover_text("Open the Nostr feed drawer")
                            .clicked()
                        {
                            self.set_nostr_feed_collapsed(ui.ctx(), false);
                        }
                        ui.add_space(6.0);
                        ui.weak("Feed");
                    });
                });
            return;
        }
        let replying = self.nostr_attached_note.is_some();
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Nostr feed");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("\u{00d7}")
                            .on_hover_text("Close the feed drawer")
                            .clicked()
                        {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui
                            .small_button("\u{203a}")
                            .on_hover_text("Collapse the feed drawer")
                            .clicked()
                        {
                            self.set_nostr_feed_collapsed(ui.ctx(), true);
                        }
                        if replying && ui.small_button("Clear reply").clicked() {
                            self.clear_nostr_attachment();
                        }
                    });
                });
                ui.weak(if replying {
                    "Compose a reply here, or bring in the AI drawer's most recent answer."
                } else {
                    "Recent notes from your external relays. Drag one onto the AI drawer, \
                     or click Attach to chat."
                });
                if let Some(status) = &self.nostr_post_status {
                    ui.weak(status);
                }
                if let Some(error) = &self.nostr_feed_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if let Some(error) = &self.nostr_post_error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if replying {
                    // Composing a reply is its own focused view, not the
                    // browse-the-feed one: the full note list (which can run
                    // to 200 entries) would just push the composer and the
                    // note being replied to off screen.
                    ui.add_space(6.0);
                    self.nostr_reply_composer(ui);
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("ATTACHED NOTE")
                            .size(10.0)
                            .strong()
                            .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
                    );
                    ui.add_space(4.0);
                    if let Some(note) = self.nostr_attached_note.clone() {
                        self.note_card(ui, &note);
                    }
                    ui.add_space(10.0);
                    ui.weak("Posting signs and publishes this reply to your configured relays.");
                } else {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .id_salt("nostr-feed-scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.nostr_feed_notes.is_empty() && self.nostr_feed_error.is_none() {
                                ui.weak("Loading recent notes\u{2026}");
                            }
                            for note in self.nostr_feed_notes.clone() {
                                self.note_card(ui, &note);
                                ui.add_space(6.0);
                            }
                        });
                }
            });
    }

    /// Resizes the feed window between its 40px collapsed sliver and its
    /// full 340px width, then re-triggers the right-edge anchoring this
    /// function already does on first map -- shrinking/growing the window
    /// moves its right edge, so it has to re-snap flush against the
    /// workspace edge rather than leaving a gap or overhanging it.
    fn set_nostr_feed_collapsed(&mut self, ctx: &egui::Context, collapsed: bool) {
        self.nostr_feed_collapsed = collapsed;
        self.nostr_feed_positioned = false;
        self.nostr_feed_position_deadline = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            if collapsed { 40.0 } else { 340.0 },
            680.0,
        )));
    }

    fn note_card(&mut self, ui: &mut egui::Ui, note: &nostr_feed_client::Note) {
        let attached = self
            .nostr_attached_note
            .as_ref()
            .is_some_and(|attached| attached.id == note.id);
        let dragging = self
            .nostr_drag_note
            .as_ref()
            .is_some_and(|dragging| dragging.id == note.id);
        let author = short_pubkey(&note.pubkey);
        let snippet: String = note.content.chars().take(220).collect();
        let mut attach = false;
        // Per the design: an attached note is a flat accent-tinted card
        // with no border (it's already the focus of the view), while an
        // ordinary feed card is the neutral surface with a hairline
        // border; dragging gets the same accent treatment as a preview.
        let frame = egui::Frame::new()
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(10, 8));
        let frame = if attached || dragging {
            frame.fill(ACCENT_CHIP)
        } else {
            frame
                .fill(ui.visuals().faint_bg_color)
                .stroke(egui::Stroke::new(1.0, LINE))
        };
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            let drag_id = ui.make_persistent_id(("nostr-note-drag", &note.id));
            let content = ui
                .scope(|ui| {
                    ui.weak(&author);
                    ui.add(egui::Label::new(snippet).wrap());
                })
                .response;
            // Sense both click and drag: a drag-only widget inside the
            // scroll area loses the hit test to the scroll background,
            // while click-and-drag is picked up reliably.
            let drag = ui.interact(content.rect, drag_id, egui::Sense::click_and_drag());
            drag.clone()
                .on_hover_text("Drag onto the AI drawer, or use Attach to chat");
            if drag.drag_started() {
                self.copy_note_and_start_drag(note, ui.ctx());
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let button = egui::Button::new(if attached {
                    "Attached"
                } else {
                    "Attach to chat"
                });
                let button = if attached {
                    button
                        .fill(ACCENT_CHIP)
                        .stroke(egui::Stroke::new(1.0, LINE))
                } else {
                    button
                };
                attach = ui.add(button).clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(
                        egui::RichText::new("Drag to AI")
                            .size(10.0)
                            .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
                    ));
                });
            });
        });
        if attach {
            self.attach_note_to_ai(note);
        }
    }

    /// Shown once a note is attached: a place to draft (with the AI's help
    /// or by hand) and then post a reply. Posting needs nothing beyond
    /// pressing Post -- you've already reviewed the text that's about to go
    /// out, same trust model as the chat's own Send button.
    fn nostr_reply_composer(&mut self, ui: &mut egui::Ui) {
        let Some(note) = self.nostr_attached_note.clone() else {
            return;
        };
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(egui::Stroke::new(1.0, LINE))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.strong(format!("Replying to {}", short_pubkey(&note.pubkey)));
                ui.horizontal(|ui| {
                    if ui.small_button("Use AI's last reply").clicked() {
                        match last_ai_reply() {
                            Some(reply) => {
                                self.nostr_compose_text = reply;
                                self.nostr_post_error = None;
                            }
                            None => {
                                self.nostr_post_error = Some(
                                    "No saved AI reply yet. Ask the AI drawer to draft one first."
                                        .into(),
                                );
                            }
                        }
                    }
                    if ui.small_button("Clear reply").clicked() {
                        self.clear_nostr_attachment();
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut self.nostr_compose_text)
                        .hint_text("Write a reply, or draft one with the AI first\u{2026}")
                        .desired_rows(4),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Signing identity")
                        .size(10.0)
                        .color(ui.visuals().weak_text_color.unwrap_or(egui::Color32::GRAY)),
                );
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("nostr-post-identity")
                        .selected_text(if self.nostr_post_identity.is_empty() {
                            "No identity loaded"
                        } else {
                            &self.nostr_post_identity
                        })
                        .show_ui(ui, |ui| {
                            for identity in self.identities.clone() {
                                ui.selectable_value(
                                    &mut self.nostr_post_identity,
                                    identity.name.clone(),
                                    &identity.name,
                                );
                            }
                        });
                    if primary_button(
                        ui,
                        "Post",
                        !self.nostr_compose_text.trim().is_empty()
                            && !self.nostr_post_identity.is_empty(),
                    ) {
                        self.post_nostr_reply(&note);
                    }
                });
            });
    }

    fn post_nostr_reply(&mut self, note: &nostr_feed_client::Note) {
        let content = self.nostr_compose_text.trim().to_string();
        if content.is_empty() || self.nostr_post_identity.is_empty() {
            return;
        }
        let tags = vec![
            vec![
                "e".to_string(),
                note.id.clone(),
                String::new(),
                "reply".to_string(),
            ],
            vec!["p".to_string(), note.pubkey.clone()],
        ];
        match signer_client::sign_event(&self.nostr_post_identity, 1, content, tags) {
            Ok(event) => {
                if let Some(handle) = &self.nostr_feed_handle {
                    handle.publish(&event);
                }
                self.nostr_post_status = Some("Posted.".into());
                self.nostr_post_error = None;
                self.nostr_compose_text.clear();
                self.nostr_attached_note = None;
            }
            Err(error) => {
                self.nostr_post_error = Some(error);
                self.nostr_post_status = None;
            }
        }
    }

    /// Copies the note text (the "drag copies to the clipboard" half) and
    /// remembers which note is being dragged for the release check.
    fn copy_note_and_start_drag(&mut self, note: &nostr_feed_client::Note, ctx: &egui::Context) {
        self.nostr_drag_note = Some(note.clone());
        self.nostr_drag_pos = ctx.input(|input| input.pointer.latest_pos());
        copy_to_clipboard(ctx, &note.content);
        self.nostr_post_status =
            Some("Copied the note. Drop it on the AI drawer to attach it.".into());
        self.nostr_post_error = None;
    }

    /// Called when the drag ends. If the release happened over the AI
    /// window, hand the note over; otherwise the clipboard copy is all the
    /// user gets, which is still useful for a normal paste.
    fn finish_note_drag(&mut self, _ctx: &egui::Context) {
        let Some(note) = self.nostr_drag_note.take() else {
            return;
        };
        let Some(position) = self.nostr_drag_pos.take() else {
            return;
        };
        let (feed, ai) = sway_windows::drawer_rects(std::process::id());
        let (Some(feed), Some(ai)) = (feed, ai) else {
            return;
        };
        if ai.contains(feed.x + position.x, feed.y + position.y) {
            self.attach_note_to_ai(&note);
        }
    }

    fn attach_note_to_ai(&mut self, note: &nostr_feed_client::Note) {
        self.nostr_attached_note = Some(note.clone());
        self.nostr_compose_text.clear();
        self.nostr_post_error = None;
        if let Err(error) = ai_handoff::write_note(note) {
            self.nostr_post_error =
                Some(format!("could not hand the note to the AI drawer: {error}"));
            return;
        }
        self.nostr_post_status = Some("Attached to the AI drawer as one-turn context.".into());
        self.ensure_ai_drawer_open();
    }

    fn ensure_ai_drawer_open(&mut self) {
        if sway_windows::window_exists("Evermore AI") {
            return;
        }
        let program = std::env::current_exe().unwrap_or_else(|_| "evermore-panel".into());
        if let Err(error) = std::process::Command::new(program).arg("--ai").spawn() {
            self.nostr_post_error = Some(format!("could not open the AI drawer: {error}"));
        }
    }

    fn clear_nostr_attachment(&mut self) {
        self.nostr_attached_note = None;
        self.nostr_compose_text.clear();
        self.nostr_post_status = None;
        self.nostr_post_error = None;
    }
}

/// Copies text to the Wayland clipboard. `wl-copy` keeps the selection
/// available after this drawer closes; `ctx.copy_text` is the fallback when
/// wl-clipboard is not installed.
fn copy_to_clipboard(ctx: &egui::Context, content: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let copied = Command::new("wl-copy")
        .args(["--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(content.as_bytes())?;
            }
            child.wait()
        })
        .is_ok_and(|status| status.success());
    if !copied {
        ctx.copy_text(content.to_owned());
    }
}

/// The most recent saved assistant turn, which is what "Use AI's last reply"
/// pulls from the AI drawer's session file.
fn last_ai_reply() -> Option<String> {
    let sessions = ai_session_client::load().ok()?;
    latest_assistant_reply(&sessions)
}

fn latest_assistant_reply(sessions: &[ai_session_client::Session]) -> Option<String> {
    sessions
        .iter()
        .max_by_key(|session| session.updated_at)
        .and_then(|session| {
            session
                .messages
                .iter()
                .rev()
                .find(|message| {
                    message.role == Role::Assistant && !message.content.trim().is_empty()
                })
                .map(|message| {
                    message
                        .content
                        .lines()
                        .filter(|line| !line.trim_start().starts_with("EVERMORE_PROPOSAL:"))
                        .collect::<Vec<_>>()
                        .join("\n")
                        .trim()
                        .to_string()
                })
        })
}

/// `abcdef0123...` -> `abcdef\u{2026}0123` for a compact byline.
fn short_pubkey(pubkey: &str) -> String {
    if pubkey.len() > 14 {
        format!("{}\u{2026}{}", &pubkey[..6], &pubkey[pubkey.len() - 6..])
    } else {
        pubkey.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evermore_common::ai::Message;

    fn session(updated_at: u64, messages: Vec<Message>) -> ai_session_client::Session {
        ai_session_client::Session {
            id: format!("session-{updated_at}"),
            name: "test".into(),
            updated_at,
            messages,
            context: Vec::new(),
        }
    }

    #[test]
    fn latest_reply_comes_from_the_newest_session() {
        let sessions = vec![
            session(
                10,
                vec![Message {
                    role: Role::Assistant,
                    content: "older reply".into(),
                }],
            ),
            session(
                20,
                vec![
                    Message {
                        role: Role::Assistant,
                        content: "newest reply".into(),
                    },
                    Message {
                        role: Role::User,
                        content: "not a reply".into(),
                    },
                ],
            ),
        ];
        assert_eq!(
            latest_assistant_reply(&sessions).as_deref(),
            Some("newest reply")
        );
    }

    #[test]
    fn empty_assistant_turns_are_skipped() {
        let sessions = vec![session(
            10,
            vec![
                Message {
                    role: Role::Assistant,
                    content: "  ".into(),
                },
                Message {
                    role: Role::Assistant,
                    content: "real reply".into(),
                },
            ],
        )];
        assert_eq!(
            latest_assistant_reply(&sessions).as_deref(),
            Some("real reply")
        );
        assert!(latest_assistant_reply(&[]).is_none());
    }
}
