use evermore_common::notifications::NotificationRecord;
use std::collections::VecDeque;
use std::sync::Mutex;

const MAX_RECORDS: usize = 200;

#[derive(Default)]
pub struct NotificationStore {
    recent: Mutex<VecDeque<NotificationRecord>>,
}

impl NotificationStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, record: NotificationRecord) {
        let mut recent = self.recent.lock().unwrap();
        recent.push_front(record);
        recent.truncate(MAX_RECORDS);
    }

    /// Most-recent-first.
    pub fn list_recent(&self) -> Vec<NotificationRecord> {
        self.recent.lock().unwrap().iter().cloned().collect()
    }
}
