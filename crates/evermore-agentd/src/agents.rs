//! Relay-backed agent definitions and their local projection.
//!
//! The owner authors each agent as an encrypted NIP-78 event
//! (`kind:30078`, `d=evermore:agent:<id>`) and publishes a NIP-51 list
//! (`kind:30000`, `d=evermore:agents`) naming the enrolled agent pubkeys.
//! `evermore-agentd` never writes those events. It projects them into a
//! private local cache so schedules survive a relay outage and validates the
//! shape before anything is scheduled.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Mutex;

use evermore_common::agent::{
    self, AGENT_D_TAG_PREFIX, AGENTS_LIST_D_TAG, AgentDefinition, AgentView, KIND_AGENT_DEFINITION,
    KIND_AGENTS_LIST,
};
use nostr::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{agent_runtime::AgentRuntimeStore, clients, relay_client};

#[derive(Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    agents: Vec<AgentDefinition>,
}

pub struct AgentStore {
    path: PathBuf,
    agents: Mutex<Vec<AgentDefinition>>,
}

impl AgentStore {
    /// Opens the default cache. A corrupt cache is reported and replaced with
    /// an empty in-memory store; the next relay reload can repopulate it.
    pub fn open() -> Result<Self, String> {
        let path = agent::agents_file_path()?;
        match Self::open_at(path.clone()) {
            Ok(store) => Ok(store),
            Err(error) => {
                eprintln!("evermore-agentd: ignoring agent cache: {error}");
                Ok(Self::empty(path))
            }
        }
    }

    pub fn open_at(path: PathBuf) -> Result<Self, String> {
        let data = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(format!("reading {}: {error}", path.display())),
        };
        let agents = if data.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice::<CacheFile>(&data)
                .map_err(|error| format!("parsing {}: {error}", path.display()))?
                .agents
        };
        Ok(Self {
            path,
            agents: Mutex::new(agents),
        })
    }

    fn empty(path: PathBuf) -> Self {
        Self {
            path,
            agents: Mutex::new(Vec::new()),
        }
    }

    pub fn list(&self) -> Vec<AgentDefinition> {
        self.agents
            .lock()
            .map(|agents| agents.clone())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<AgentDefinition> {
        self.agents
            .lock()
            .ok()
            .and_then(|agents| agents.iter().find(|agent| agent.id == id).cloned())
    }

    pub fn replace_all(&self, mut agents: Vec<AgentDefinition>) -> Result<(), String> {
        agents.sort_by(|a, b| a.id.cmp(&b.id));
        agents.dedup_by(|a, b| a.id == b.id);
        // The periodic relay refresh runs every 30 seconds; avoid rewriting
        // the cache (and waking disk) when nothing actually changed.
        if self.list() == agents {
            return Ok(());
        }
        self.persist(&agents)?;
        let mut guard = self
            .agents
            .lock()
            .map_err(|_| "agent cache lock poisoned".to_string())?;
        *guard = agents;
        Ok(())
    }

    /// Returns each cached definition with a live check of whether its
    /// signing identity exists on this device and matches the claimed pubkey.
    pub fn views(
        &self,
        owner_pubkey: &str,
        assistant_pubkey: &str,
        runtime: &AgentRuntimeStore,
    ) -> Vec<AgentView> {
        self.list()
            .into_iter()
            .map(|definition| {
                let (available, reason) = if definition.agent_pubkey == owner_pubkey {
                    (
                        false,
                        Some("agent identity must differ from the owner".to_string()),
                    )
                } else if definition.agent_pubkey == assistant_pubkey {
                    (
                        false,
                        Some("agent identity duplicates the interactive assistant".to_string()),
                    )
                } else {
                    match clients::signer_public_key(&definition.identity) {
                        Ok(pubkey) if pubkey == definition.agent_pubkey => (true, None),
                        Ok(pubkey) => (
                            false,
                            Some(format!(
                                "identity '{}' resolves to {pubkey}, not {}",
                                definition.identity, definition.agent_pubkey
                            )),
                        ),
                        Err(error) => (
                            false,
                            Some(format!(
                                "identity '{}' unavailable: {error}",
                                definition.identity
                            )),
                        ),
                    }
                };
                AgentView {
                    runtime: runtime.snapshot(&definition.id),
                    definition,
                    available,
                    unavailable_reason: reason,
                }
            })
            .collect()
    }

    /// Queries the relay, projects the definitions, and replaces the cache.
    pub fn reload(
        &self,
        relay_url: &str,
        owner_pubkey: &str,
        owner_identity: &str,
        assistant_pubkey: &str,
        runtime: &AgentRuntimeStore,
    ) -> Result<agent::AgentReload, String> {
        let definitions = relay_client::query(
            relay_url,
            serde_json::json!({
                "kinds": [KIND_AGENT_DEFINITION],
                "authors": [owner_pubkey],
            }),
        )?;
        let lists = relay_client::query(
            relay_url,
            serde_json::json!({
                "kinds": [KIND_AGENTS_LIST],
                "authors": [owner_pubkey],
                "#d": [AGENTS_LIST_D_TAG],
            }),
        )?;
        if definitions.is_empty() && lists.is_empty() {
            return Ok(agent::AgentReload {
                agents: self.views(owner_pubkey, assistant_pubkey, runtime),
                warnings: vec![
                    "the relay returned no agent events; keeping the local cache".to_string(),
                ],
            });
        }
        let (agents, warnings) =
            project_events(&definitions, &lists, owner_pubkey, |ciphertext| {
                clients::nip44_decrypt(owner_identity, owner_pubkey, ciphertext)
            })?;
        self.replace_all(agents)?;
        Ok(agent::AgentReload {
            agents: self.views(owner_pubkey, assistant_pubkey, runtime),
            warnings,
        })
    }

    fn persist(&self, agents: &[AgentDefinition]) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "agent cache path has no parent directory".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("setting permissions on {}: {error}", parent.display()))?;

        let temporary = parent.join(format!(".agents-{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec(&CacheFile {
            agents: agents.to_vec(),
        })
        .map_err(|error| error.to_string())?;
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
            std::fs::rename(&temporary, &self.path)
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("writing {}: {error}", self.path.display()));
        }
        Ok(())
    }
}

/// Pure projection step: takes relay events, decrypts the definitions, keeps
/// the latest event per agent id, and filters by the NIP-51 membership list.
/// Kept separate from `AgentStore` so it is testable without a relay or a
/// signer.
pub fn project_events<F>(
    definitions: &[Event],
    lists: &[Event],
    owner_pubkey: &str,
    decrypt_definition: F,
) -> Result<(Vec<AgentDefinition>, Vec<String>), String>
where
    F: Fn(&str) -> Result<String, String>,
{
    let mut warnings = Vec::new();
    let membership = membership(lists, owner_pubkey, &mut warnings);

    let mut latest: BTreeMap<String, Event> = BTreeMap::new();
    for event in definitions {
        if event.kind != Kind::from(KIND_AGENT_DEFINITION) || event.pubkey.to_hex() != owner_pubkey
        {
            continue;
        }
        let Some(tag) = event
            .tags
            .iter()
            .find(|tag| tag.as_slice().first().map(String::as_str) == Some("d"))
        else {
            continue;
        };
        let Some(d_tag) = tag.as_slice().get(1) else {
            continue;
        };
        let Some(id) = d_tag.strip_prefix(AGENT_D_TAG_PREFIX) else {
            continue;
        };
        if event.verify().is_err() {
            warnings.push(format!("agent '{id}': event signature is invalid"));
            continue;
        }
        let replace = latest
            .get(id)
            .map(|current| event.created_at.as_secs() > current.created_at.as_secs())
            .unwrap_or(true);
        if replace {
            latest.insert(id.to_string(), event.clone());
        }
    }

    let mut agents = Vec::new();
    for (id, event) in latest {
        let plaintext = match decrypt_definition(&event.content) {
            Ok(plaintext) => plaintext,
            Err(error) => {
                warnings.push(format!(
                    "agent '{id}': couldn't decrypt definition: {error}"
                ));
                continue;
            }
        };
        let definition: AgentDefinition = match serde_json::from_str(&plaintext) {
            Ok(definition) => definition,
            Err(error) => {
                warnings.push(format!("agent '{id}': invalid definition JSON: {error}"));
                continue;
            }
        };
        if definition.id != id {
            warnings.push(format!(
                "agent '{id}': definition says id '{}'",
                definition.id
            ));
            continue;
        }
        if let Err(error) = definition.validate() {
            warnings.push(format!("agent '{id}': {error}"));
            continue;
        }
        if let Some(members) = &membership
            && !members.contains(&definition.agent_pubkey)
        {
            continue;
        }
        agents.push(definition);
    }
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((agents, warnings))
}

fn membership(
    lists: &[Event],
    owner_pubkey: &str,
    warnings: &mut Vec<String>,
) -> Option<BTreeSet<String>> {
    let latest = lists
        .iter()
        .filter(|event| {
            event.kind == Kind::from(KIND_AGENTS_LIST)
                && event.pubkey.to_hex() == owner_pubkey
                && event.tags.iter().any(|tag| {
                    tag.as_slice().first().map(String::as_str) == Some("d")
                        && tag.as_slice().get(1).map(String::as_str) == Some(AGENTS_LIST_D_TAG)
                })
        })
        .max_by_key(|event| event.created_at.as_secs())?;
    if latest.verify().is_err() {
        warnings.push("agent list: event signature is invalid".to_string());
        return None;
    }
    let members = latest
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some("p"))
        .filter_map(|tag| tag.as_slice().get(1).cloned())
        .collect::<BTreeSet<_>>();
    Some(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        keys: &Keys,
        kind: u16,
        content: &str,
        tags: Vec<Vec<String>>,
        created_at: u64,
    ) -> Event {
        let mut builder = EventBuilder::new(Kind::from(kind), content)
            .custom_created_at(Timestamp::from(created_at));
        for tag in tags {
            builder = builder.tag(Tag::parse(tag).unwrap());
        }
        builder.finalize(keys).unwrap()
    }

    fn definition_json(id: &str, identity: &str, pubkey: &str) -> String {
        serde_json::json!({
            "version": 1,
            "id": id,
            "title": "Morning digest",
            "description": "",
            "identity": identity,
            "agent_pubkey": pubkey,
            "enabled": true,
            "jobs": [{
                "id": "digest",
                "recipe_id": "audit.digest",
                "inputs": {},
                "interval_seconds": 86400,
                "enabled": true
            }],
            "created_at": 1,
            "updated_at": 1
        })
        .to_string()
    }

    #[test]
    fn projects_latest_definition_and_filters_by_membership() {
        let owner = Keys::generate();
        let owner_pubkey = owner.public_key().to_hex();
        let member_pubkey = "a".repeat(64);
        let other_pubkey = "b".repeat(64);

        let older = event(
            &owner,
            KIND_AGENT_DEFINITION,
            "cipher-old",
            vec![vec![
                "d".to_string(),
                format!("{AGENT_D_TAG_PREFIX}morning-digest"),
            ]],
            10,
        );
        let newer = event(
            &owner,
            KIND_AGENT_DEFINITION,
            "cipher-new",
            vec![vec![
                "d".to_string(),
                format!("{AGENT_D_TAG_PREFIX}morning-digest"),
            ]],
            20,
        );
        let excluded = event(
            &owner,
            KIND_AGENT_DEFINITION,
            "cipher-other",
            vec![vec![
                "d".to_string(),
                format!("{AGENT_D_TAG_PREFIX}other-agent"),
            ]],
            20,
        );
        let list = event(
            &owner,
            KIND_AGENTS_LIST,
            "",
            vec![
                vec!["d".to_string(), AGENTS_LIST_D_TAG.to_string()],
                vec!["p".to_string(), member_pubkey.clone()],
            ],
            20,
        );

        let decrypt_definition = |ciphertext: &str| match ciphertext {
            "cipher-new" => Ok(definition_json(
                "morning-digest",
                "agent-morning-digest",
                &member_pubkey,
            )),
            "cipher-old" => Ok(definition_json(
                "morning-digest",
                "agent-morning-digest",
                &other_pubkey,
            )),
            "cipher-other" => Ok(definition_json(
                "other-agent",
                "agent-other-agent",
                &other_pubkey,
            )),
            _ => Err("no key".to_string()),
        };

        let (agents, warnings) = project_events(
            &[older, newer, excluded],
            &[list],
            &owner_pubkey,
            decrypt_definition,
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "morning-digest");
        assert_eq!(agents[0].agent_pubkey, member_pubkey);
    }

    #[test]
    fn warns_and_skips_undecryptable_or_mismatched_definitions() {
        let owner = Keys::generate();
        let owner_pubkey = owner.public_key().to_hex();
        let broken = event(
            &owner,
            KIND_AGENT_DEFINITION,
            "cipher-broken",
            vec![vec!["d".to_string(), format!("{AGENT_D_TAG_PREFIX}broken")]],
            10,
        );
        let mismatch = event(
            &owner,
            KIND_AGENT_DEFINITION,
            "cipher-mismatch",
            vec![vec![
                "d".to_string(),
                format!("{AGENT_D_TAG_PREFIX}mismatch"),
            ]],
            10,
        );
        let decrypt_definition = |ciphertext: &str| match ciphertext {
            "cipher-broken" => Err("no key".to_string()),
            "cipher-mismatch" => Ok(definition_json(
                "different-id",
                "agent-different-id",
                &"a".repeat(64),
            )),
            _ => unreachable!(),
        };
        let (agents, warnings) =
            project_events(&[broken, mismatch], &[], &owner_pubkey, decrypt_definition).unwrap();
        assert!(agents.is_empty());
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn cache_roundtrips_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents/agents.json");
        let store = AgentStore::open_at(path.clone()).unwrap();
        assert!(store.list().is_empty());

        let agent = AgentDefinition {
            version: 1,
            id: "morning-digest".into(),
            title: "Morning digest".into(),
            description: String::new(),
            identity: "agent-morning-digest".into(),
            agent_pubkey: "a".repeat(64),
            enabled: true,
            jobs: vec![agent::AgentJob {
                id: "digest".into(),
                recipe_id: "audit.digest".into(),
                inputs: serde_json::json!({}),
                interval_seconds: 86_400,
                enabled: true,
            }],
            created_at: 0,
            updated_at: 0,
        };
        store.replace_all(vec![agent.clone()]).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let reopened = AgentStore::open_at(path).unwrap();
        assert_eq!(reopened.list(), vec![agent]);
    }
}
