use evermore_common::notifications::NotificationRecord;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_RECORDS: usize = 200;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub struct NotificationStore {
    recent: Mutex<VecDeque<NotificationRecord>>,
}

impl NotificationStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, mut record: NotificationRecord) {
        record.id = format!(
            "{}-{}",
            record.received_at,
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut recent = self.recent.lock().unwrap();
        recent.push_front(record);
        recent.truncate(MAX_RECORDS);
    }

    /// Most-recent-first.
    pub fn list_recent(&self) -> Vec<NotificationRecord> {
        self.recent.lock().unwrap().iter().cloned().collect()
    }

    pub fn clear_recent(&self) -> usize {
        let mut recent = self.recent.lock().unwrap();
        let removed = recent.len();
        recent.clear();
        removed
    }

    pub fn dismiss(&self, notification_id: &str) -> Result<bool, String> {
        let mut recent = self
            .recent
            .lock()
            .map_err(|_| "notification lock poisoned")?;
        let Some(index) = recent
            .iter()
            .position(|record| record.id == notification_id)
        else {
            return Ok(false);
        };
        if recent[index].source == evermore_common::notifications::Source::Dm {
            return Err("AI operation cannot dismiss a private-message notification".into());
        }
        recent.remove(index);
        Ok(true)
    }
}
