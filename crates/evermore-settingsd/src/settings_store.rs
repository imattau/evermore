//! In-memory settings cache, one JSON object per domain. Populated by
//! decrypting incoming kind:30078 events from the relay; read/written by
//! the local Unix socket server.

use std::collections::HashMap;
use std::sync::Mutex;

pub const KIND_SETTINGS: u16 = 30078;

/// `d`-tag prefix for Evermore settings domains, so the relay's addressable
/// (kind 30000-39999) replacement semantics key on this consistently.
pub fn d_tag(domain: &str) -> String {
    format!("evermore:{domain}")
}

#[derive(Default)]
pub struct SettingsStore {
    domains: Mutex<HashMap<String, serde_json::Map<String, serde_json::Value>>>,
}

impl SettingsStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces a domain's whole key/value map (called after decrypting an
    /// incoming settings event -- kind:30078 is addressable, so the latest
    /// event for a `d` tag replaces whatever we had cached).
    pub fn replace_domain(&self, domain: &str, map: serde_json::Map<String, serde_json::Value>) {
        self.domains.lock().unwrap().insert(domain.to_string(), map);
    }

    pub fn get(&self, domain: &str, key: &str) -> Option<serde_json::Value> {
        self.domains
            .lock()
            .unwrap()
            .get(domain)
            .and_then(|m| m.get(key))
            .cloned()
    }

    pub fn get_domain(&self, domain: &str) -> serde_json::Map<String, serde_json::Value> {
        self.domains
            .lock()
            .unwrap()
            .get(domain)
            .cloned()
            .unwrap_or_default()
    }

    pub fn list_domains(&self) -> Vec<String> {
        let mut names: Vec<String> = self.domains.lock().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    /// Sets one key within a domain, returning the domain's full updated
    /// map (the caller re-encrypts and republishes this whole map, since
    /// that's what the single addressable event per domain holds).
    pub fn set(
        &self,
        domain: &str,
        key: &str,
        value: serde_json::Value,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut domains = self.domains.lock().unwrap();
        let map = domains.entry(domain.to_string()).or_default();
        map.insert(key.to_string(), value);
        map.clone()
    }
}
