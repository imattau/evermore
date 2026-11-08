//! One-note handoff from the Nostr feed drawer to the AI drawer.
//!
//! The two drawers are separate native windows (and processes), so an egui
//! drag cannot cross between them: the source window keeps the pointer grab
//! and the destination never receives a drop event. The feed therefore
//! writes the note it wants the AI to see here, and the AI drawer consumes
//! it on its next frame. Writes go through a temporary file and rename so a
//! half-written snapshot can never be read.

use crate::nostr_feed_client::Note;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where the feed leaves a note for the AI drawer.
pub fn handoff_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("evermore/ai-note-handoff.json")
}

pub fn write_note(note: &Note) -> Result<(), String> {
    write_note_to(&handoff_path(), note)
}

/// Reads and removes the pending note, if any. The AI drawer calls this once
/// per frame; removing on read means the same note is attached once.
pub fn take_note() -> Option<Note> {
    take_note_from(&handoff_path())
}

fn write_note_to(path: &Path, note: &Note) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| "AI handoff path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut file, note).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        error.to_string()
    })
}

fn take_note_from(path: &Path) -> Option<Note> {
    let bytes = std::fs::read(path).ok()?;
    let _ = std::fs::remove_file(path);
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note() -> Note {
        Note {
            id: "0123456789abcdef".into(),
            pubkey: "fedcba9876543210".into(),
            content: "hello from the feed".into(),
            created_at: 1_760_000_000,
        }
    }

    fn scratch_path(name: &str) -> PathBuf {
        let unique = format!(
            "evermore-ai-handoff-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        std::env::temp_dir().join(unique).join("handoff.json")
    }

    #[test]
    fn handoff_round_trips_one_note() {
        let path = scratch_path("round-trip");
        write_note_to(&path, &note()).expect("write handoff");
        let taken = take_note_from(&path).expect("take handoff");
        assert_eq!(taken.id, note().id);
        assert_eq!(taken.content, note().content);
        assert!(
            take_note_from(&path).is_none(),
            "taking the note should consume it"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_handoff_is_not_an_error() {
        let path = scratch_path("missing");
        assert!(take_note_from(&path).is_none());
    }
}
