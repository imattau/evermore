use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use evermore_common::operations::{self, Access};
use nostr::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const REQUEST_TTL_SECS: u64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Approved,
    Running,
    Denied,
    Expired,
    Succeeded,
    Failed,
    ExecutionUnknown,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    pub operation_id: String,
    pub actor_pubkey: String,
    pub operation: String,
    pub operation_version: u32,
    pub arguments: Value,
    pub request_event: Value,
    pub approval_event: Option<Value>,
    #[serde(default)]
    pub request_published: bool,
    #[serde(default)]
    pub approval_published: bool,
    #[serde(default)]
    pub result_event: Option<Value>,
    #[serde(default)]
    pub result_published: bool,
    pub status: Status,
    pub result: Option<Value>,
    pub error: Option<String>,
    #[serde(default)]
    pub relay_error: Option<String>,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct StoreFile {
    operations: BTreeMap<String, OperationRecord>,
    #[serde(default)]
    grants: BTreeMap<String, GrantRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantRecord {
    pub grant_id: String,
    pub agent_pubkey: String,
    pub allowed_operations: Vec<String>,
    pub expires_at: u64,
    pub grant_event: Value,
    pub grant_published: bool,
    pub revoked: bool,
    pub revocation_event: Option<Value>,
    pub revocation_published: bool,
}

#[derive(Debug, Clone)]
pub struct PendingGrantEvent {
    pub grant_id: String,
    pub revocation: bool,
    pub event: Value,
}

pub struct OperationStore {
    path: PathBuf,
    owner_pubkey: PublicKey,
    data: Mutex<StoreFile>,
}

#[derive(Clone, Copy)]
pub enum RelayPhase {
    Request,
    Approval,
    Result,
}

impl OperationStore {
    pub fn open(owner_pubkey: PublicKey) -> Result<Self, String> {
        let path = data_path()?;
        let parent = path
            .parent()
            .ok_or_else(|| "operation data path has no parent directory".to_string())?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let mut data: StoreFile = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("couldn't parse operation store {}: {e}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreFile::default(),
            Err(error) => return Err(error.to_string()),
        };
        let mut recovered = false;
        for record in data.operations.values_mut() {
            if record.status == Status::Running {
                // The side effect may or may not have completed before the
                // previous process stopped. Never replay it automatically.
                record.status = Status::ExecutionUnknown;
                record.error = Some(
                    "daemon restarted after execution was claimed; outcome requires review".into(),
                );
                recovered = true;
            }
        }
        let store = Self {
            path,
            owner_pubkey,
            data: Mutex::new(data),
        };
        if recovered {
            let data = store
                .data
                .lock()
                .map_err(|_| "operation store lock poisoned")?;
            store.persist(&data)?;
        }
        Ok(store)
    }

    pub fn submit(
        &self,
        event_value: Value,
        plaintext_content: &str,
    ) -> Result<OperationRecord, String> {
        let event: Event = serde_json::from_value(event_value.clone())
            .map_err(|e| format!("invalid signed request event: {e}"))?;
        event
            .verify()
            .map_err(|e| format!("invalid request signature: {e}"))?;
        require_fresh_event(&event)?;
        if event.kind != Kind::TextNote {
            return Err("operation requests must use a signed kind:1 event".to_string());
        }
        if !has_tag(&event, "t", "evermore-operation-request") {
            return Err("missing operation request marker tag".to_string());
        }
        if !has_tag(&event, "p", &self.owner_pubkey.to_hex()) {
            return Err("request must identify the configured owner in a p tag".to_string());
        }

        let payload: Value = serde_json::from_str(plaintext_content)
            .map_err(|e| format!("request content must be JSON: {e}"))?;
        require_exact_keys(&payload, &["operation", "arguments"], "request")?;
        let name = payload
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| "request is missing operation name".to_string())?;
        let arguments = payload
            .get("arguments")
            .cloned()
            .ok_or_else(|| "request is missing arguments".to_string())?;
        let definition =
            operations::find(name).ok_or_else(|| format!("unknown operation '{name}'"))?;
        if definition.access != Access::Write
            || definition.approval != operations::Approval::Required
        {
            return Err(format!("'{name}' is not an approval-gated write operation"));
        }
        operations::validate_input(name, &arguments)?;

        let operation_id = event.id.to_hex();
        let now = unix_time();
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let actor_pubkey = event.pubkey.to_hex();
        if event.pubkey != self.owner_pubkey && !has_operation_grant(&data, &actor_pubkey, name) {
            return Err(format!(
                "agent {actor_pubkey} has no active grant for '{name}'"
            ));
        }
        if let Some(existing) = data.operations.get(&operation_id) {
            return Ok(existing.clone());
        }
        let record = OperationRecord {
            operation_id: operation_id.clone(),
            actor_pubkey,
            operation: name.to_string(),
            operation_version: definition.version,
            arguments,
            request_event: event_value,
            approval_event: None,
            request_published: false,
            approval_published: false,
            result_event: None,
            result_published: false,
            status: Status::Pending,
            result: None,
            error: None,
            relay_error: None,
            created_at: now,
            expires_at: now + REQUEST_TTL_SECS,
        };
        data.operations.insert(operation_id, record.clone());
        self.persist(&data)?;
        Ok(record)
    }

    pub fn register_grant(&self, event_value: Value) -> Result<GrantRecord, String> {
        let event: Event = serde_json::from_value(event_value.clone())
            .map_err(|e| format!("invalid grant event: {e}"))?;
        event
            .verify()
            .map_err(|e| format!("invalid grant signature: {e}"))?;
        require_fresh_event(&event)?;
        if event.kind != Kind::TextNote || event.pubkey != self.owner_pubkey {
            return Err("agent grants must be signed by the configured owner using kind:1".into());
        }
        if !has_tag(&event, "t", "evermore-agent-grant") {
            return Err("missing agent grant marker tag".into());
        }
        let payload: Value = serde_json::from_str(&event.content)
            .map_err(|e| format!("grant content must be JSON: {e}"))?;
        require_exact_keys(
            &payload,
            &["agent_pubkey", "allowed_operations", "expires_at"],
            "grant",
        )?;
        let agent_pubkey = payload
            .get("agent_pubkey")
            .and_then(Value::as_str)
            .ok_or_else(|| "grant is missing agent_pubkey".to_string())?;
        let agent_key = PublicKey::parse(agent_pubkey).map_err(|e| e.to_string())?;
        if agent_key == self.owner_pubkey {
            return Err("the owner identity cannot be granted an agent role".into());
        }
        if !has_tag(&event, "p", &agent_key.to_hex()) {
            return Err("grant p tag must match agent_pubkey".into());
        }
        let allowed_operations = payload
            .get("allowed_operations")
            .and_then(Value::as_array)
            .ok_or_else(|| "allowed_operations must be an array".to_string())?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| "every allowed operation must be a string".to_string())
                    .map(str::to_string)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if allowed_operations.is_empty() {
            return Err("agent grant must allow at least one operation".into());
        }
        let mut unique = std::collections::BTreeSet::new();
        for name in &allowed_operations {
            if operations::find(name).is_none() {
                return Err(format!("unknown operation in grant: '{name}'"));
            }
            if !unique.insert(name) {
                return Err(format!("duplicate operation in grant: '{name}'"));
            }
        }
        let expires_at = payload
            .get("expires_at")
            .and_then(Value::as_u64)
            .ok_or_else(|| "expires_at must be a Unix timestamp".to_string())?;
        let now = unix_time();
        if expires_at <= now || expires_at > now + 365 * 24 * 60 * 60 {
            return Err("grant expiry must be within the next 365 days".into());
        }
        let grant_id = event.id.to_hex();
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        if let Some(existing) = data.grants.get(&grant_id) {
            return Ok(existing.clone());
        }
        let grant = GrantRecord {
            grant_id: grant_id.clone(),
            agent_pubkey: agent_key.to_hex(),
            allowed_operations,
            expires_at,
            grant_event: event_value,
            grant_published: false,
            revoked: false,
            revocation_event: None,
            revocation_published: false,
        };
        data.grants.insert(grant_id, grant.clone());
        self.persist(&data)?;
        Ok(grant)
    }

    pub fn revoke_grant(&self, event_value: Value) -> Result<GrantRecord, String> {
        let event: Event = serde_json::from_value(event_value.clone())
            .map_err(|e| format!("invalid grant revocation event: {e}"))?;
        event
            .verify()
            .map_err(|e| format!("invalid grant revocation signature: {e}"))?;
        require_fresh_event(&event)?;
        if event.kind != Kind::TextNote || event.pubkey != self.owner_pubkey {
            return Err("agent grant revocations must be signed by the configured owner".into());
        }
        if !has_tag(&event, "t", "evermore-agent-grant-revoke") {
            return Err("missing grant revocation marker tag".into());
        }
        let grant_id = event
            .tags
            .iter()
            .find(|tag| tag.as_slice().first().map(String::as_str) == Some("e"))
            .and_then(|tag| tag.as_slice().get(1))
            .cloned()
            .ok_or_else(|| {
                "grant revocation must reference a grant event with an e tag".to_string()
            })?;
        let payload: Value = serde_json::from_str(&event.content)
            .map_err(|e| format!("grant revocation content must be JSON: {e}"))?;
        require_exact_keys(&payload, &["grant_id"], "grant revocation")?;
        if payload.get("grant_id").and_then(Value::as_str) != Some(&grant_id) {
            return Err("grant revocation content and e tag IDs do not match".into());
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let agent_pubkey = {
            let grant = data
                .grants
                .get_mut(&grant_id)
                .ok_or_else(|| "unknown grant_id".to_string())?;
            grant.revoked = true;
            grant.revocation_event = Some(event_value);
            grant.revocation_published = false;
            grant.agent_pubkey.clone()
        };
        for operation in data.operations.values_mut() {
            if operation.actor_pubkey == agent_pubkey
                && matches!(operation.status, Status::Pending | Status::Approved)
            {
                operation.status = Status::Revoked;
                operation.error = Some("agent grant was revoked before execution".into());
            }
        }
        let revoked = data.grants.get(&grant_id).expect("grant exists").clone();
        self.persist(&data)?;
        Ok(revoked)
    }

    pub fn grants(&self) -> Result<Vec<GrantRecord>, String> {
        let data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        Ok(data.grants.values().cloned().collect())
    }

    pub fn approve(&self, event_value: Value) -> Result<OperationRecord, String> {
        let event: Event = serde_json::from_value(event_value.clone())
            .map_err(|e| format!("invalid approval event: {e}"))?;
        event
            .verify()
            .map_err(|e| format!("invalid approval signature: {e}"))?;
        require_fresh_event(&event)?;
        if event.kind != Kind::TextNote {
            return Err("operation approvals must use a signed kind:1 event".to_string());
        }
        if event.pubkey != self.owner_pubkey {
            return Err("approval must be signed by the configured owner identity".to_string());
        }
        if !has_tag(&event, "t", "evermore-operation-approval") {
            return Err("missing operation approval marker tag".to_string());
        }
        let operation_id = event
            .tags
            .iter()
            .find(|tag| tag.as_slice().first().map(String::as_str) == Some("e"))
            .and_then(|tag| tag.as_slice().get(1))
            .cloned()
            .ok_or_else(|| "approval must reference the request event with an e tag".to_string())?;
        let payload: Value = serde_json::from_str(&event.content)
            .map_err(|e| format!("approval content must be JSON: {e}"))?;
        require_exact_keys(&payload, &["operation_id", "decision"], "approval")?;
        if payload.get("operation_id").and_then(Value::as_str) != Some(&operation_id) {
            return Err("approval content and e tag operation IDs do not match".to_string());
        }
        let decision = payload
            .get("decision")
            .and_then(Value::as_str)
            .ok_or_else(|| "approval decision must be approve or deny".to_string())?;
        let next_status = match decision {
            "approve" => Status::Approved,
            "deny" => Status::Denied,
            _ => return Err("approval decision must be approve or deny".to_string()),
        };

        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let existing = data
            .operations
            .get(&operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        if existing.actor_pubkey != self.owner_pubkey.to_hex() {
            if event.pubkey.to_hex() == existing.actor_pubkey {
                return Err("an agent cannot approve its own operation request".into());
            }
            if !has_operation_grant(&data, &existing.actor_pubkey, &existing.operation) {
                return Err("agent grant is expired or revoked".into());
            }
        }
        let record = data
            .operations
            .get_mut(&operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        if record.status != Status::Pending {
            if record
                .approval_event
                .as_ref()
                .and_then(|e| e.get("id"))
                .and_then(Value::as_str)
                == Some(&event.id.to_hex())
            {
                return Ok(record.clone());
            }
            return Err(format!("operation is already {:?}", record.status).to_lowercase());
        }
        if unix_time() >= record.expires_at {
            record.status = Status::Expired;
            self.persist(&data)?;
            return Err("operation approval has expired".to_string());
        }
        record.status = next_status;
        record.approval_event = Some(event_value);
        record.approval_published = false;
        let result = record.clone();
        self.persist(&data)?;
        Ok(result)
    }

    pub fn status(&self, operation_id: &str) -> Result<OperationRecord, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let record = data
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        if record.status == Status::Pending && unix_time() >= record.expires_at {
            record.status = Status::Expired;
            self.persist(&data)?;
        }
        data.operations
            .get(operation_id)
            .cloned()
            .ok_or_else(|| "unknown operation_id".to_string())
    }

    pub fn expire_legacy_plaintext_request(
        &self,
        operation_id: &str,
    ) -> Result<OperationRecord, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let record = data
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        record.status = Status::Expired;
        record.error = Some(
            "legacy plaintext request was not published or executed; resubmit with encrypted request content".into(),
        );
        let expired = record.clone();
        self.persist(&data)?;
        Ok(expired)
    }

    pub fn claim_execution(&self, operation_id: &str) -> Result<OperationRecord, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let existing = data
            .operations
            .get(operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        if existing.status != Status::Approved {
            return Err(format!("operation is already {:?}", existing.status).to_lowercase());
        }
        if !existing.request_published || !existing.approval_published {
            return Err(
                "request and approval must be acknowledged by the local relay before execution"
                    .into(),
            );
        }
        if existing.actor_pubkey != self.owner_pubkey.to_hex()
            && !has_operation_grant(&data, &existing.actor_pubkey, &existing.operation)
        {
            let record = data
                .operations
                .get_mut(operation_id)
                .expect("record exists");
            record.status = Status::Revoked;
            record.error = Some("agent grant expired or was revoked before execution".into());
            self.persist(&data)?;
            return Err("agent grant expired or was revoked before execution".into());
        }
        let record = data
            .operations
            .get_mut(operation_id)
            .expect("record exists");
        record.status = Status::Running;
        record.error = None;
        let claimed = record.clone();
        self.persist(&data)?;
        Ok(claimed)
    }

    pub fn all_records(&self) -> Result<Vec<OperationRecord>, String> {
        let data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        Ok(data.operations.values().cloned().collect())
    }

    pub fn pending_grant_events(&self) -> Result<Vec<PendingGrantEvent>, String> {
        let data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let mut pending = Vec::new();
        for grant in data.grants.values() {
            if !grant.grant_published {
                pending.push(PendingGrantEvent {
                    grant_id: grant.grant_id.clone(),
                    revocation: false,
                    event: grant.grant_event.clone(),
                });
            }
            if let Some(event) = &grant.revocation_event
                && !grant.revocation_published
            {
                pending.push(PendingGrantEvent {
                    grant_id: grant.grant_id.clone(),
                    revocation: true,
                    event: event.clone(),
                });
            }
        }
        Ok(pending)
    }

    pub fn mark_grant_published(
        &self,
        grant_id: &str,
        revocation: bool,
    ) -> Result<GrantRecord, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let grant = data
            .grants
            .get_mut(grant_id)
            .ok_or_else(|| "unknown grant_id".to_string())?;
        if revocation {
            grant.revocation_published = true;
        } else {
            grant.grant_published = true;
        }
        let updated = grant.clone();
        self.persist(&data)?;
        Ok(updated)
    }

    pub fn mark_published(
        &self,
        operation_id: &str,
        phase: RelayPhase,
    ) -> Result<OperationRecord, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let record = data
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        match phase {
            RelayPhase::Request => record.request_published = true,
            RelayPhase::Approval => record.approval_published = true,
            RelayPhase::Result => record.result_published = true,
        }
        let updated = record.clone();
        self.persist(&data)?;
        Ok(updated)
    }

    pub fn save_result_event(
        &self,
        operation_id: &str,
        event: Value,
    ) -> Result<OperationRecord, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let record = data
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        if record.result_event.is_none() {
            record.result_event = Some(event);
            record.result_published = false;
        }
        let updated = record.clone();
        self.persist(&data)?;
        Ok(updated)
    }

    pub fn set_relay_error(
        &self,
        operation_id: &str,
        error: Option<String>,
    ) -> Result<OperationRecord, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let record = data
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        record.relay_error = error;
        let updated = record.clone();
        self.persist(&data)?;
        Ok(updated)
    }

    pub fn finish_execution(
        &self,
        operation_id: &str,
        result: Result<Value, String>,
    ) -> Result<OperationRecord, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let record = data
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        if record.status != Status::Running {
            return Err(format!("operation is not running ({:?})", record.status).to_lowercase());
        }
        match result {
            Ok(value) => {
                let definition = operations::find(&record.operation)
                    .ok_or_else(|| "operation definition was removed".to_string())?;
                record.result = Some(operations::redact_result(&definition, value));
                record.status = Status::Succeeded;
            }
            Err(error) => {
                record.error = Some(error);
                record.status = Status::Failed;
            }
        }
        let finished = record.clone();
        self.persist(&data)?;
        Ok(finished)
    }

    pub fn mark_execution_unknown(
        &self,
        operation_id: &str,
        error: String,
    ) -> Result<OperationRecord, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "operation store lock poisoned".to_string())?;
        let record = data
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| "unknown operation_id".to_string())?;
        if record.status != Status::Running {
            return Err(format!("operation is not running ({:?})", record.status).to_lowercase());
        }
        record.status = Status::ExecutionUnknown;
        record.error = Some(error);
        let finished = record.clone();
        self.persist(&data)?;
        Ok(finished)
    }

    fn persist(&self, data: &StoreFile) -> Result<(), String> {
        let parent = self.path.parent().expect("store path has a parent");
        let temp_path = parent.join(format!("operations.json.{}.tmp", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp_path)
            .map_err(|e| e.to_string())?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        let encoded = serde_json::to_vec(data).map_err(|e| e.to_string())?;
        file.write_all(&encoded).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&temp_path, &self.path).map_err(|e| e.to_string())?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn has_tag(event: &Event, tag_name: &str, expected: &str) -> bool {
    event.tags.iter().any(|tag| {
        let tag = tag.as_slice();
        tag.first().map(String::as_str) == Some(tag_name)
            && tag.get(1).map(String::as_str) == Some(expected)
    })
}

fn has_operation_grant(data: &StoreFile, agent_pubkey: &str, operation: &str) -> bool {
    data.grants.values().any(|grant| {
        grant.agent_pubkey == agent_pubkey
            && grant.grant_published
            && !grant.revoked
            && unix_time() < grant.expires_at
            && grant
                .allowed_operations
                .iter()
                .any(|allowed| allowed == operation)
    })
}

fn require_fresh_event(event: &Event) -> Result<(), String> {
    let now = unix_time();
    let created_at = event.created_at.as_secs();
    if created_at > now.saturating_add(60) {
        return Err("signed operation event timestamp is too far in the future".to_string());
    }
    if now.saturating_sub(created_at) > REQUEST_TTL_SECS {
        return Err("signed operation event has expired".to_string());
    }
    Ok(())
}

fn require_exact_keys(value: &Value, expected: &[&str], label: &str) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label} content must be a JSON object"))?;
    if object.len() != expected.len() || object.keys().any(|key| !expected.contains(&key.as_str()))
    {
        return Err(format!(
            "{label} content must contain only {}",
            expected.join(" and ")
        ));
    }
    Ok(())
}

fn data_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())?;
    Ok(base.join("evermore/operations/operations.json"))
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
