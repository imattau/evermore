//! Wire protocol shared between `evermore-signer` and its clients.
//!
//! The socket speaks newline-delimited JSON: one [`Request`] per line in,
//! one [`Response`] per line out. Kept free of any `nostr`-crate types so
//! clients don't have to pull in signing/crypto code just to talk to the
//! daemon -- event payloads cross the wire as plain JSON.

use serde::{Deserialize, Serialize};

/// Path (relative to `$XDG_RUNTIME_DIR`) of the signer's Unix socket.
pub const SOCKET_RELATIVE_PATH: &str = "evermore/signer.sock";

/// Resolves the signer socket path under `$XDG_RUNTIME_DIR` (falling back to
/// `/tmp` if unset, mainly so this doesn't panic outside a real session).
pub fn runtime_socket_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    std::path::Path::new(&base).join(SOCKET_RELATIVE_PATH)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    /// List the identity names currently loaded in the daemon.
    ListIdentities,
    /// Get an identity's public key (hex and bech32/npub).
    GetPublicKey { identity: String },
    /// Build and sign a Nostr event under the given identity, returning the
    /// full signed event as JSON (id, pubkey, sig, etc. all filled in).
    SignEvent {
        identity: String,
        kind: u16,
        content: String,
        #[serde(default)]
        tags: Vec<Vec<String>>,
        /// Unix timestamp; defaults to "now" if omitted.
        #[serde(skip_serializing_if = "Option::is_none")]
        created_at: Option<u64>,
    },
    /// NIP-44 encrypt `plaintext` from `identity` to `recipient_pubkey` (hex).
    Nip44Encrypt {
        identity: String,
        recipient_pubkey: String,
        plaintext: String,
    },
    /// NIP-44 decrypt `ciphertext` received by `identity` from `sender_pubkey` (hex).
    Nip44Decrypt {
        identity: String,
        sender_pubkey: String,
        ciphertext: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { data: serde_json::Value },
    Err { message: String },
}

/// The shared, declarative operation catalogue used to expose Evermore
/// capabilities to local clients and (later) MCP. Catalogue entries describe
/// permissions and schemas; they are not executable authority by themselves.
pub mod operations {
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};

    pub const SOCKET_RELATIVE_PATH: &str = "evermore/operationsd.sock";

    pub fn runtime_socket_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        std::path::Path::new(&base).join(SOCKET_RELATIVE_PATH)
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        Catalog,
        GetDefinition { name: String },
        Invoke { name: String, arguments: Value },
        SubmitSignedRequest { event: Value },
        SubmitApproval { event: Value },
        OperationStatus { operation_id: String },
        ListPendingOperations,
        OperationHistory { limit: usize },
        ExportSnapshot,
        SubmitGrant { event: Value },
        RevokeGrant { event: Value },
        ListGrants,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "status", rename_all = "snake_case")]
    pub enum Response {
        Ok { data: Value },
        Err { message: String },
    }

    impl Response {
        pub fn ok(data: impl Serialize) -> Self {
            Response::Ok {
                data: serde_json::to_value(data).unwrap_or(Value::Null),
            }
        }

        pub fn err(message: impl Into<String>) -> Self {
            Response::Err {
                message: message.into(),
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Access {
        Read,
        Write,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Approval {
        None,
        Required,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Redaction {
        None,
        SecretShapedValues,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Definition {
        pub name: String,
        pub version: u32,
        pub description: String,
        pub access: Access,
        pub required_grants: Vec<String>,
        pub approval: Approval,
        pub input_schema: Value,
        pub output_schema: Value,
        pub result_redaction: Redaction,
    }

    /// Build the current catalogue in stable name order. Interfaces should
    /// consume these definitions rather than duplicating tool metadata.
    pub fn catalog() -> Vec<Definition> {
        let mut definitions = vec![
            definition(
                "identity.get_public_key",
                "Return the public key for a loaded identity; never returns a secret key.",
                Access::Read,
                "identity.read",
                Approval::None,
                object_schema(
                    json!({
                        "identity": { "type": "string", "minLength": 1 }
                    }),
                    &["identity"],
                ),
                object_schema(
                    json!({
                        "pubkey_hex": { "type": "string" },
                        "npub": { "type": "string" }
                    }),
                    &["pubkey_hex", "npub"],
                ),
                Redaction::None,
            ),
            definition(
                "notifications.list_recent",
                "List recent notification metadata.",
                Access::Read,
                "notifications.read",
                Approval::None,
                object_schema(json!({}), &[]),
                json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "source": { "type": "string", "enum": ["local", "mention"] },
                            "title": { "type": "string" },
                            "body": { "type": "string" },
                            "received_at": { "type": "integer", "minimum": 0 }
                        },
                        "required": ["id", "source", "title", "body", "received_at"],
                        "additionalProperties": false
                    }
                }),
                Redaction::SecretShapedValues,
            ),
            definition(
                "notifications.clear_recent",
                "Clear recent notifications from the local in-memory notification view. Requires owner approval.",
                Access::Write,
                "notifications.write",
                Approval::Required,
                object_schema(json!({}), &[]),
                object_schema(
                    json!({ "cleared": { "type": "integer", "minimum": 0 } }),
                    &["cleared"],
                ),
                Redaction::None,
            ),
            definition(
                "notifications.dismiss",
                "Dismiss one current non-DM notification by its local notification ID. Requires owner approval.",
                Access::Write,
                "notifications.write",
                Approval::Required,
                object_schema(
                    json!({ "notification_id": { "type": "string", "minLength": 1, "maxLength": 128 } }),
                    &["notification_id"],
                ),
                object_schema(
                    json!({ "dismissed": { "type": "boolean" } }),
                    &["dismissed"],
                ),
                Redaction::None,
            ),
            definition(
                "packages.install_pinned",
                "Propose installation or update to one exact package version from a trusted, signed npack release. The archive hash must match before the current package version changes.",
                Access::Write,
                "packages.write",
                Approval::Required,
                object_schema(
                    json!({
                        "package_id": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" },
                        "version": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" },
                        "sha256": { "type": "string", "format": "sha256" }
                    }),
                    &["package_id", "version", "sha256"],
                ),
                object_schema(
                    json!({ "package_id": { "type": "string" }, "version": { "type": "string" }, "sha256": { "type": "string" } }),
                    &["package_id", "version", "sha256"],
                ),
                Redaction::None,
            ),
            definition(
                "packages.list_installed",
                "List locally installed npack packages and their manifest metadata.",
                Access::Read,
                "packages.read",
                Approval::None,
                object_schema(json!({}), &[]),
                json!({
                    "type": "array",
                    "items": { "type": "object" }
                }),
                Redaction::SecretShapedValues,
            ),
            definition(
                "packages.list_available",
                "Discover the latest package versions and archive hashes from publishers in the owner's trusted-publisher list. Use these values to explain updates; installation still requires an exact owner-approved proposal.",
                Access::Read,
                "packages.read",
                Approval::None,
                object_schema(json!({}), &[]),
                json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "publisher_pubkey": { "type": "string" },
                            "version": { "type": ["string", "null"] },
                            "sha256": { "type": ["string", "null"] }
                        },
                        "required": ["id", "name", "description", "publisher_pubkey", "version", "sha256"],
                        "additionalProperties": false
                    }
                }),
                Redaction::SecretShapedValues,
            ),
            definition(
                "blossom.list_owned",
                "List content-addressed blobs owned by the configured package identity on the configured Blossom server.",
                Access::Read,
                "blossom.read",
                Approval::None,
                object_schema(json!({}), &[]),
                json!({ "type": "array", "items": { "type": "object" } }),
                Redaction::None,
            ),
            definition(
                "blossom.delete",
                "Propose deletion of one blob owned by the configured identity. Ownership is checked against the server listing before deletion.",
                Access::Write,
                "blossom.write",
                Approval::Required,
                object_schema(
                    json!({ "sha256": { "type": "string", "format": "sha256" } }),
                    &["sha256"],
                ),
                object_schema(
                    json!({ "sha256": { "type": "string" }, "deleted": { "type": "boolean" } }),
                    &["sha256", "deleted"],
                ),
                Redaction::None,
            ),
            definition(
                "packages.explain_dependencies",
                "Explain a package's declared direct dependencies and whether each is installed.",
                Access::Read,
                "packages.read",
                Approval::None,
                object_schema(
                    json!({ "package_id": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" } }),
                    &["package_id"],
                ),
                object_schema(
                    json!({
                        "package_id": { "type": "string" },
                        "name": { "type": "string" },
                        "version": { "type": "string" },
                        "dependencies": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "package_id": { "type": "string" },
                                    "installed": { "type": "boolean" },
                                    "name": { "type": ["string", "null"] },
                                    "version": { "type": ["string", "null"] }
                                },
                                "required": ["package_id", "installed", "name", "version"],
                                "additionalProperties": false
                            }
                        }
                    }),
                    &["package_id", "name", "version", "dependencies"],
                ),
                Redaction::None,
            ),
            definition(
                "packages.remove",
                "Propose removal of one installed npack package and its package-managed executable link. Requires owner approval.",
                Access::Write,
                "packages.write",
                Approval::Required,
                object_schema(
                    json!({ "package_id": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" } }),
                    &["package_id"],
                ),
                object_schema(
                    json!({ "package_id": { "type": "string" }, "removed": { "type": "boolean" } }),
                    &["package_id", "removed"],
                ),
                Redaction::None,
            ),
            definition(
                "ai.model.install_signed_npack",
                "Import the declared GGUF (and optional LoRA adapter) from an already installed signed npack package into the local model store. Requires owner approval.",
                Access::Write,
                "packages.write",
                Approval::Required,
                object_schema(
                    json!({ "package_id": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" } }),
                    &["package_id"],
                ),
                object_schema(
                    json!({ "package_id": { "type": "string" }, "model": { "type": "object" } }),
                    &["package_id", "model"],
                ),
                Redaction::None,
            ),
            definition(
                "settings.get",
                "Read one value from a settings domain.",
                Access::Read,
                "settings.read",
                Approval::None,
                object_schema(
                    json!({
                        "domain": { "type": "string", "minLength": 1 },
                        "key": { "type": "string", "minLength": 1 }
                    }),
                    &["domain", "key"],
                ),
                json!({}),
                Redaction::SecretShapedValues,
            ),
            definition(
                "settings.get_domain",
                "Read a settings domain.",
                Access::Read,
                "settings.read",
                Approval::None,
                object_schema(
                    json!({
                        "domain": { "type": "string", "minLength": 1 }
                    }),
                    &["domain"],
                ),
                json!({ "type": "object" }),
                Redaction::SecretShapedValues,
            ),
            definition(
                "settings.list_domains",
                "List settings domains containing values.",
                Access::Read,
                "settings.read",
                Approval::None,
                object_schema(json!({}), &[]),
                json!({ "type": "array", "items": { "type": "string" } }),
                Redaction::None,
            ),
            definition(
                "ai.context.list_sources",
                "List AI context sources enabled for this user session.",
                Access::Read,
                "ai.context.read",
                Approval::None,
                object_schema(json!({}), &[]),
                json!({ "type": "array", "items": { "type": "string" } }),
                Redaction::None,
            ),
            definition(
                "ai.context.get",
                "Read one context source enabled by the user's local context policy.",
                Access::Read,
                "ai.context.read",
                Approval::None,
                object_schema(
                    json!({
                    "source": { "type": "string", "enum": ["compositor", "settings:theme", "notifications:local", "notifications:mention", "packages:installed"] }
                    }),
                    &["source"],
                ),
                json!({ "type": "object" }),
                Redaction::SecretShapedValues,
            ),
            definition(
                "ai.index.search",
                "Search locally indexed, explicitly selected text files.",
                Access::Read,
                "ai.index.read",
                Approval::None,
                object_schema(
                    json!({
                        "query": { "type": "string", "minLength": 1, "maxLength": 8192 },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 20 }
                    }),
                    &["query"],
                ),
                json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "scope_id": { "type": "string" },
                            "path": { "type": "string" },
                            "excerpt": { "type": "string" },
                            "score": { "type": "number" }
                        },
                        "required": ["scope_id", "path", "excerpt", "score"],
                        "additionalProperties": false
                    }
                }),
                Redaction::SecretShapedValues,
            ),
            definition(
                "sway.list_windows",
                "List window metadata from the active Sway session.",
                Access::Read,
                "sway.read",
                Approval::None,
                object_schema(json!({}), &[]),
                json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "con_id": { "type": "integer", "minimum": 1 },
                            "title": { "type": "string" },
                            "app_id": { "type": "string" },
                            "window_class": { "type": "string" },
                            "workspace": { "type": "string" },
                            "focused": { "type": "boolean" }
                        },
                        "required": ["con_id", "title", "app_id", "window_class", "workspace", "focused"],
                        "additionalProperties": false
                    }
                }),
                Redaction::SecretShapedValues,
            ),
            definition(
                "sway.focus_window",
                "Focus a window by its current Sway container ID. Requires owner approval.",
                Access::Write,
                "sway.write",
                Approval::Required,
                object_schema(
                    json!({ "con_id": { "type": "integer", "minimum": 1 } }),
                    &["con_id"],
                ),
                object_schema(
                    json!({ "operation_id": { "type": "string" } }),
                    &["operation_id"],
                ),
                Redaction::None,
            ),
            definition(
                "sway.switch_workspace",
                "Switch to a numbered workspace from 1 to 10. Requires owner approval.",
                Access::Write,
                "sway.write",
                Approval::Required,
                object_schema(
                    json!({ "workspace_number": { "type": "integer", "minimum": 1, "maximum": 10 } }),
                    &["workspace_number"],
                ),
                object_schema(
                    json!({ "operation_id": { "type": "string" } }),
                    &["operation_id"],
                ),
                Redaction::None,
            ),
            definition(
                "sway.launch_app",
                "Launch an application by executable name and argument vector. The owner reviews the exact executable and arguments before launch; no shell is invoked.",
                Access::Write,
                "sway.write",
                Approval::Required,
                object_schema(
                    json!({
                        "program": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" },
                        "args": { "type": "array", "maxItems": 32, "items": { "type": "string", "maxLength": 1024 } }
                    }),
                    &["program", "args"],
                ),
                object_schema(
                    json!({ "program": { "type": "string" }, "started": { "type": "boolean" }, "pid": { "type": "integer", "minimum": 1 } }),
                    &["program", "started", "pid"],
                ),
                Redaction::None,
            ),
            definition(
                "settings.set",
                "Set a setting value. Requires owner approval before execution.",
                Access::Write,
                "settings.write",
                Approval::Required,
                object_schema(
                    json!({
                        "domain": { "type": "string", "minLength": 1 },
                        "key": { "type": "string", "minLength": 1 },
                        "value": {}
                    }),
                    &["domain", "key", "value"],
                ),
                object_schema(
                    json!({ "operation_id": { "type": "string" } }),
                    &["operation_id"],
                ),
                Redaction::SecretShapedValues,
            ),
        ];
        definitions.sort_by(|a, b| a.name.cmp(&b.name));
        definitions
    }

    pub fn find(name: &str) -> Option<Definition> {
        catalog()
            .into_iter()
            .find(|definition| definition.name == name)
    }

    /// Validate an operation argument object against the catalogue's JSON
    /// Schema subset (required fields, additional properties, primitive
    /// types, string length, and numeric bounds).
    pub fn validate_input(name: &str, arguments: &Value) -> Result<(), String> {
        let definition = find(name).ok_or_else(|| format!("unknown operation '{name}'"))?;
        validate_schema(&definition.input_schema, arguments, "arguments")
    }

    /// Validate and produce the stable JSON value used by every interface
    /// when creating or invoking an operation. Object keys are normalized
    /// recursively; array order and scalar values remain unchanged.
    pub fn canonicalize_input(name: &str, arguments: &Value) -> Result<Value, String> {
        validate_input(name, arguments)?;
        Ok(canonicalize_value(arguments))
    }

    fn canonicalize_value(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut normalized = serde_json::Map::new();
                let mut keys: Vec<_> = object.keys().collect();
                keys.sort();
                for key in keys {
                    normalized.insert(key.clone(), canonicalize_value(&object[key]));
                }
                Value::Object(normalized)
            }
            Value::Array(items) => Value::Array(items.iter().map(canonicalize_value).collect()),
            other => other.clone(),
        }
    }

    /// Sanitize model-visible data according to the operation's declared
    /// policy. The human-facing UI can separately request the raw local data.
    pub fn redact_result(definition: &Definition, mut value: Value) -> Value {
        if definition.result_redaction == Redaction::SecretShapedValues {
            redact_value(&mut value, None);
        }
        value
    }

    /// Read a single operation record from the private durable journal when
    /// the operation daemon is unavailable. This is read-only: expired
    /// pending requests and interrupted execution claims are reflected in the
    /// returned snapshot but are not written back.
    pub fn read_local_record(operation_id: &str) -> Result<Value, String> {
        let data = read_local_journal()?;
        let record = data
            .get("operations")
            .and_then(Value::as_object)
            .and_then(|records| records.get(operation_id))
            .cloned()
            .ok_or_else(|| "unknown operation_id in local journal".to_string())?;
        Ok(normalize_offline_record(record))
    }

    /// Read recent journal records without requiring the daemon socket.
    pub fn read_local_history(limit: usize) -> Result<Vec<Value>, String> {
        let data = read_local_journal()?;
        let mut records: Vec<Value> = data
            .get("operations")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|records| records.values())
            .cloned()
            .map(normalize_offline_record)
            .collect();
        records.sort_by_key(|record| {
            std::cmp::Reverse(
                record
                    .get("created_at")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            )
        });
        records.truncate(limit.clamp(1, 100));
        Ok(records)
    }

    /// Read pending requests from the journal while the executor is stopped.
    pub fn read_local_pending() -> Result<Vec<Value>, String> {
        Ok(read_local_history(100)?
            .into_iter()
            .filter(|record| record.get("status").and_then(Value::as_str) == Some("pending"))
            .collect())
    }

    fn read_local_journal() -> Result<Value, String> {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| std::path::PathBuf::from(home).join(".local/share"))
            })
            .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())?;
        let path = base.join("evermore/operations/operations.json");
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "couldn't read local operation journal {}: {error}",
                path.display()
            )
        })?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(
                "local operation journal must be a regular file with private permissions".into(),
            );
        }
        let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
        serde_json::from_slice(&bytes)
            .map_err(|error| format!("couldn't parse local operation journal: {error}"))
    }

    fn normalize_offline_record(mut record: Value) -> Value {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let status = record.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "pending"
            && record
                .get("expires_at")
                .and_then(Value::as_u64)
                .is_some_and(|expires_at| now >= expires_at)
        {
            record["status"] = Value::String("expired".into());
        } else if status == "running" {
            record["status"] = Value::String("execution_unknown".into());
            record["error"] = Value::String("operation daemon is unavailable after an execution claim; outcome requires review and must not be replayed".into());
        }
        record["status_source"] = Value::String("local_journal_offline".into());
        record
    }

    fn redact_value(value: &mut Value, key: Option<&str>) {
        const SECRET_KEY_PARTS: &[&str] = &[
            "secret",
            "token",
            "password",
            "credential",
            "private_key",
            "nsec",
            "api_key",
        ];
        if key.is_some_and(|key| {
            let normalized = key.to_ascii_lowercase();
            SECRET_KEY_PARTS
                .iter()
                .any(|part| normalized.contains(part))
        }) {
            *value = Value::String("[REDACTED]".to_string());
            return;
        }
        match value {
            Value::Object(object) => {
                for (key, child) in object.iter_mut() {
                    redact_value(child, Some(key));
                }
            }
            Value::Array(items) => {
                for item in items {
                    redact_value(item, None);
                }
            }
            Value::String(text) if looks_secret_shaped(text) => {
                *value = Value::String("[REDACTED]".to_string());
            }
            _ => {}
        }
    }

    fn looks_secret_shaped(value: &str) -> bool {
        let lower = value.to_ascii_lowercase();
        lower.starts_with("nsec1")
            || lower.starts_with("ncryptsec1")
            || lower.starts_with("bearer ")
            || (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    }

    fn validate_schema(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
        if let Some(expected_type) = schema.get("type").and_then(Value::as_str) {
            let valid = match expected_type {
                "object" => value.is_object(),
                "array" => value.is_array(),
                "string" => value.is_string(),
                "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
                "number" => value.is_number(),
                "boolean" => value.is_boolean(),
                _ => true,
            };
            if !valid {
                return Err(format!("{path} must be {expected_type}"));
            }
        }

        if let Some(object) = value.as_object() {
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(key) {
                        return Err(format!("{path}.{key} is required"));
                    }
                }
            }
            if schema.get("additionalProperties") == Some(&Value::Bool(false))
                && let Some(properties) = schema.get("properties").and_then(Value::as_object)
                && let Some(key) = object.keys().find(|key| !properties.contains_key(*key))
            {
                return Err(format!("{path}.{key} is not allowed"));
            }
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (key, child_schema) in properties {
                    if let Some(child_value) = object.get(key) {
                        validate_schema(child_schema, child_value, &format!("{path}.{key}"))?;
                    }
                }
            }
        }

        if let Some(text) = value.as_str() {
            if schema.get("format").and_then(Value::as_str) == Some("sha256")
                && (text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()))
            {
                return Err(format!("{path} must be a 64-character hexadecimal SHA-256"));
            }
            if schema.get("format").and_then(Value::as_str) == Some("path_component")
                && (text == "."
                    || text == ".."
                    || text.is_empty()
                    || !text
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte)))
            {
                return Err(format!(
                    "{path} contains characters unsafe for a package path"
                ));
            }
            if schema
                .get("minLength")
                .and_then(Value::as_u64)
                .is_some_and(|n| text.chars().count() < n as usize)
            {
                return Err(format!("{path} is too short"));
            }
            if schema
                .get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|n| text.chars().count() > n as usize)
            {
                return Err(format!("{path} is too long"));
            }
            if let Some(options) = schema.get("enum").and_then(Value::as_array)
                && !options.iter().any(|option| option == value)
            {
                return Err(format!("{path} is not an allowed value"));
            }
        }
        if let Some(number) = value.as_f64() {
            if schema
                .get("minimum")
                .and_then(Value::as_f64)
                .is_some_and(|min| number < min)
            {
                return Err(format!("{path} is below the minimum"));
            }
            if schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|max| number > max)
            {
                return Err(format!("{path} is above the maximum"));
            }
        }
        if let Some(array) = value.as_array() {
            if schema
                .get("minItems")
                .and_then(Value::as_u64)
                .is_some_and(|minimum| array.len() < minimum as usize)
            {
                return Err(format!("{path} has too few items"));
            }
            if schema
                .get("maxItems")
                .and_then(Value::as_u64)
                .is_some_and(|maximum| array.len() > maximum as usize)
            {
                return Err(format!("{path} has too many items"));
            }
        }
        if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
            for (index, item) in array.iter().enumerate() {
                validate_schema(items, item, &format!("{path}[{index}]"))?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // internal catalogue-construction helper, not public API
    fn definition(
        name: &str,
        description: &str,
        access: Access,
        grant: &str,
        approval: Approval,
        input_schema: Value,
        output_schema: Value,
        result_redaction: Redaction,
    ) -> Definition {
        Definition {
            name: name.to_string(),
            version: 1,
            description: description.to_string(),
            access,
            required_grants: vec![grant.to_string()],
            approval,
            input_schema,
            output_schema,
            result_redaction,
        }
    }

    fn object_schema(properties: Value, required: &[&str]) -> Value {
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn app_launch_arguments_are_bounded_and_canonical() {
            let valid = json!({ "program": "example", "args": ["--safe", "value"] });
            assert_eq!(
                canonicalize_input("sway.launch_app", &valid).unwrap(),
                valid
            );

            let too_many = json!({
                "program": "example",
                "args": (0..33).map(|index| format!("arg-{index}")).collect::<Vec<_>>()
            });
            assert!(
                canonicalize_input("sway.launch_app", &too_many)
                    .unwrap_err()
                    .contains("too many items")
            );

            let too_long = json!({ "program": "example", "args": ["x".repeat(1025)] });
            assert!(
                canonicalize_input("sway.launch_app", &too_long)
                    .unwrap_err()
                    .contains("too long")
            );
        }
    }
}

/// Local inference service protocol. Requests and response events are newline
/// delimited JSON on the private `ai-inferd` Unix socket.
pub mod ai {
    use serde::{Deserialize, Serialize};
    use std::path::{Component, Path, PathBuf};

    pub const INFER_SOCKET_RELATIVE_PATH: &str = "evermore/ai-inferd.sock";
    /// Bumped from 1 when inference moved from a shared Ollama backend to a
    /// supervised `llama-server` pool. Fields added to `Model` and `Health`
    /// are additive, but the unload/lifecycle semantics changed.
    pub const PROTOCOL_VERSION: u32 = 2;
    pub const MODEL_MANIFEST_VERSION: u32 = 1;

    pub fn runtime_infer_socket_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        std::path::Path::new(&base).join(INFER_SOCKET_RELATIVE_PATH)
    }

    /// Redact common credential-shaped values before selected text is embedded
    /// or shown to a model. This is a safety filter, not a substitute for the
    /// user's explicit source selection and scope policy.
    pub fn redact_text(text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        let mut in_private_key = false;
        let mut redact_next = false;
        for line in text.split_inclusive('\n') {
            let upper = line.to_ascii_uppercase();
            if upper.contains("-----BEGIN") && upper.contains("PRIVATE KEY-----") {
                output.push_str("[REDACTED PRIVATE KEY BLOCK]\n");
                in_private_key = true;
                continue;
            }
            if in_private_key {
                if upper.contains("-----END") && upper.contains("PRIVATE KEY-----") {
                    in_private_key = false;
                }
                continue;
            }
            let mut start = 0;
            for (index, character) in line.char_indices() {
                if character.is_whitespace() {
                    let token = &line[start..index];
                    output.push_str(&redact_text_token(token, &mut redact_next));
                    output.push(character);
                    start = index + character.len_utf8();
                }
            }
            output.push_str(&redact_text_token(&line[start..], &mut redact_next));
        }
        output
    }

    fn redact_text_token(token: &str, redact_next: &mut bool) -> String {
        let lower = token.to_ascii_lowercase();
        const SECRET_LABELS: &[&str] = &[
            "password",
            "passwd",
            "token",
            "api_key",
            "apikey",
            "secret",
            "credential",
            "private_key",
        ];
        if *redact_next && !token.is_empty() {
            *redact_next = false;
            return "[REDACTED]".into();
        }
        let trimmed = token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
        let normalized = trimmed.to_ascii_lowercase();
        let has_assignment = token.split_once('=').is_some();
        if SECRET_LABELS.iter().any(|label| normalized == *label) && !has_assignment {
            *redact_next = true;
            return token.to_string();
        }
        if let Some((key, value)) = token.split_once('=')
            && SECRET_LABELS.iter().any(|label| {
                key.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                    .eq_ignore_ascii_case(label)
            })
            && !value.is_empty()
        {
            return format!("{key}[REDACTED]");
        }
        let nsec = lower.starts_with("nsec1") || lower.starts_with("ncryptsec1");
        let bearer_token = lower == "bearer";
        let hex_secret =
            trimmed.len() == 64 && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit());
        if bearer_token {
            *redact_next = true;
            token.to_string()
        } else if nsec || hex_secret {
            "[REDACTED]".into()
        } else {
            token.to_string()
        }
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Backend {
        #[default]
        Auto,
        Cpu,
        Gpu,
    }

    /// One file inside a model package, identified by its content hash.
    /// Hashes are verified when a signed package is imported; the store scan
    /// checks structure and file presence, not gigabytes of GGUF on every
    /// startup.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ModelArtifact {
        pub file: String,
        pub sha256: String,
        #[serde(default)]
        pub size_bytes: Option<u64>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ModelAdapter {
        pub file: String,
        pub sha256: String,
        #[serde(default)]
        pub size_bytes: Option<u64>,
        #[serde(default = "default_adapter_scale")]
        pub scale: f32,
    }

    /// The on-disk manifest written next to a base GGUF and its optional LoRA
    /// adapter. The base + adapter pair is the versioned artifact; adapters
    /// are bound to the operation-registry version they were trained against.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ModelManifest {
        pub manifest_version: u32,
        pub package_id: String,
        pub package_version: String,
        #[serde(default)]
        pub description: String,
        pub base: ModelArtifact,
        #[serde(default)]
        pub adapter: Option<ModelAdapter>,
        pub context_size: u32,
        #[serde(default)]
        pub quant: String,
        /// `chat` and/or `embeddings`; empty is treated as `chat` for
        /// compatibility with manifests written before capabilities existed.
        #[serde(default)]
        pub capabilities: Vec<String>,
        #[serde(default)]
        pub operation_registry_version: Option<String>,
        #[serde(default)]
        pub publisher: Option<String>,
    }

    impl ModelManifest {
        pub fn name(&self) -> String {
            format!("evermore/{}:{}", self.package_id, self.package_version)
        }

        pub fn supports(&self, capability: &str) -> bool {
            if self.capabilities.is_empty() {
                return capability == "chat";
            }
            self.capabilities.iter().any(|item| item == capability)
        }

        /// Structural validation performed before a manifest is trusted for a
        /// launch. Hash verification is the importer's job (M7.13); this
        /// refuses unsafe paths, malformed digests, and unpaired adapters.
        pub fn validate(&self) -> Result<(), String> {
            if self.manifest_version != MODEL_MANIFEST_VERSION {
                return Err(format!(
                    "unsupported model manifest version {}",
                    self.manifest_version
                ));
            }
            if !is_safe_component(&self.package_id, 128) {
                return Err("model package id is unsafe or empty".into());
            }
            if !is_safe_component(&self.package_version, 64) {
                return Err("model package version is unsafe or empty".into());
            }
            validate_artifact(&self.base, "base")?;
            if let Some(adapter) = &self.adapter {
                validate_artifact(
                    &ModelArtifact {
                        file: adapter.file.clone(),
                        sha256: adapter.sha256.clone(),
                        size_bytes: adapter.size_bytes,
                    },
                    "adapter",
                )?;
                if !adapter.scale.is_finite() || !(0.0..=4.0).contains(&adapter.scale) {
                    return Err("model adapter scale must be between 0 and 4".into());
                }
                match &self.operation_registry_version {
                    Some(version) if !version.trim().is_empty() => {}
                    _ => {
                        return Err(
                            "a model adapter must declare the operation-registry version it was trained against"
                                .into(),
                        );
                    }
                }
            }
            if !(512..=131_072).contains(&self.context_size) {
                return Err("model context size must be between 512 and 131072".into());
            }
            for capability in &self.capabilities {
                if capability != "chat" && capability != "embeddings" {
                    return Err(format!("unsupported model capability '{capability}'"));
                }
            }
            Ok(())
        }
    }

    fn validate_artifact(artifact: &ModelArtifact, label: &str) -> Result<(), String> {
        let path = Path::new(&artifact.file);
        let mut components = path.components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(format!("model {label} file must be a plain filename"));
        }
        if artifact.sha256.len() != 64
            || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!("model {label} SHA-256 is malformed"));
        }
        Ok(())
    }

    fn is_safe_component(value: &str, max_len: usize) -> bool {
        !value.is_empty()
            && value.len() <= max_len
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
    }

    /// Model roots in priority order: the system store from `npack`, then the
    /// per-user store. `EVERMORE_MODEL_DIRS` (colon-separated) overrides both.
    pub fn model_store_roots() -> Vec<PathBuf> {
        if let Some(spec) = std::env::var_os("EVERMORE_MODEL_DIRS") {
            let roots = parse_model_store_roots(&spec.to_string_lossy());
            if !roots.is_empty() {
                return roots;
            }
        }
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        vec![
            PathBuf::from("/var/lib/evermore/models"),
            data_home.join("evermore/models"),
        ]
    }

    pub fn parse_model_store_roots(spec: &str) -> Vec<PathBuf> {
        spec.split(':')
            .filter(|part| !part.trim().is_empty())
            .map(|part| PathBuf::from(part.trim()))
            .collect()
    }

    fn default_adapter_scale() -> f32 {
        1.0
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Role {
        System,
        User,
        Assistant,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Message {
        pub role: Role,
        pub content: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Model {
        pub name: String,
        pub digest: Option<String>,
        pub size_bytes: Option<u64>,
        pub modified_at: Option<String>,
        #[serde(default)]
        pub quant: Option<String>,
        #[serde(default)]
        pub context_size: Option<u32>,
        #[serde(default)]
        pub adapter: Option<String>,
        #[serde(default)]
        pub capabilities: Vec<String>,
        #[serde(default)]
        pub operation_registry_version: Option<String>,
    }

    /// Runtime state of a LoRA adapter loaded by a model's `llama-server`.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct AdapterState {
        pub id: u32,
        pub name: String,
        pub scale: f32,
        pub loaded: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        Health,
        ListModels,
        ListAdapters {
            model: String,
        },
        SetAdapterScale {
            model: String,
            scale: f32,
        },
        Chat {
            request_id: String,
            model: String,
            messages: Vec<Message>,
            #[serde(default = "default_backend")]
            backend: Backend,
            #[serde(default)]
            max_tokens: Option<u32>,
            #[serde(default)]
            temperature: Option<f32>,
            /// Optional GBNF grammar constraining generated text. Used by
            /// operation-proposal callers that need parseable JSON.
            #[serde(default)]
            grammar: Option<String>,
        },
        Embed {
            request_id: String,
            model: String,
            inputs: Vec<String>,
        },
        UnloadModel {
            model: String,
        },
        InstallNpackModel {
            package_id: String,
        },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "event", rename_all = "snake_case")]
    pub enum Event {
        Health {
            protocol_version: u32,
            backend: String,
            ready: bool,
        },
        Models {
            models: Vec<Model>,
        },
        Adapters {
            model: String,
            adapters: Vec<AdapterState>,
        },
        ModelInstalled {
            package_id: String,
            model: Model,
        },
        Embeddings {
            request_id: String,
            model: String,
            vectors: Vec<Vec<f32>>,
            prompt_tokens: Option<u64>,
            total_duration_ns: Option<u64>,
        },
        Started {
            request_id: String,
            model: String,
            backend_requested: Backend,
        },
        Token {
            request_id: String,
            text: String,
        },
        Completed {
            request_id: String,
            backend_used: Backend,
            prompt_tokens: Option<u64>,
            completion_tokens: Option<u64>,
            total_duration_ns: Option<u64>,
        },
        Error {
            request_id: Option<String>,
            message: String,
        },
    }

    fn default_backend() -> Backend {
        Backend::Auto
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn manifest(adapter: bool) -> ModelManifest {
            ModelManifest {
                manifest_version: MODEL_MANIFEST_VERSION,
                package_id: "p7.model.tinyllama".into(),
                package_version: "1.0.0".into(),
                description: "test".into(),
                base: ModelArtifact {
                    file: "base.gguf".into(),
                    sha256: "a".repeat(64),
                    size_bytes: None,
                },
                adapter: adapter.then(|| ModelAdapter {
                    file: "adapter.gguf".into(),
                    sha256: "b".repeat(64),
                    size_bytes: None,
                    scale: 1.0,
                }),
                context_size: 2048,
                quant: "Q4_K_M".into(),
                capabilities: vec!["chat".into()],
                operation_registry_version: adapter.then(|| "2026-11-08".into()),
                publisher: None,
            }
        }

        #[test]
        fn manifest_round_trips_and_validates() {
            let value = manifest(true);
            value.validate().unwrap();
            let bytes = serde_json::to_vec(&value).unwrap();
            let parsed: ModelManifest = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(parsed.name(), "evermore/p7.model.tinyllama:1.0.0");
            assert!(parsed.supports("chat"));
            assert!(!parsed.supports("embeddings"));
        }

        #[test]
        fn adapter_requires_registry_version() {
            let mut value = manifest(true);
            value.operation_registry_version = None;
            assert!(value.validate().unwrap_err().contains("operation-registry"));
        }

        #[test]
        fn unsafe_artifact_paths_are_refused() {
            let mut value = manifest(false);
            value.base.file = "../escape.gguf".into();
            assert!(value.validate().is_err());
            value.base.file = "nested/base.gguf".into();
            assert!(value.validate().is_err());
        }

        #[test]
        fn empty_capabilities_default_to_chat() {
            let mut value = manifest(false);
            value.capabilities.clear();
            assert!(value.supports("chat"));
            assert!(!value.supports("embeddings"));
        }

        #[test]
        fn store_roots_parse_priority_order() {
            let roots =
                parse_model_store_roots("/system/models:/home/u/.local/share/evermore/models");
            assert_eq!(roots.len(), 2);
            assert_eq!(roots[0], PathBuf::from("/system/models"));
            assert!(parse_model_store_roots("").is_empty());
        }

        #[test]
        fn chat_grammar_is_optional_on_the_wire() {
            let legacy: Request = serde_json::from_value(serde_json::json!({
                "method": "chat",
                "params": {
                    "request_id": "r",
                    "model": "evermore/m:1",
                    "messages": [{ "role": "user", "content": "hi" }]
                }
            }))
            .unwrap();
            match legacy {
                Request::Chat {
                    grammar, backend, ..
                } => {
                    assert!(grammar.is_none());
                    assert_eq!(backend, Backend::Auto);
                }
                other => panic!("unexpected request: {other:?}"),
            }

            let with_grammar: Request = serde_json::from_value(serde_json::json!({
                "method": "chat",
                "params": {
                    "request_id": "r",
                    "model": "evermore/m:1",
                    "messages": [{ "role": "user", "content": "hi" }],
                    "grammar": "root ::= \"{}\""
                }
            }))
            .unwrap();
            match with_grammar {
                Request::Chat { grammar, .. } => {
                    assert_eq!(grammar.as_deref(), Some("root ::= \"{}\""));
                }
                other => panic!("unexpected request: {other:?}"),
            }
        }
    }
}

/// Read-only local semantic index API. Scope configuration remains an
/// explicit user-side setting; model-facing requests can only search or
/// reindex already selected files.
pub mod ai_index {
    use serde::{Deserialize, Serialize};

    pub const SOCKET_RELATIVE_PATH: &str = "evermore/ai-indexd.sock";
    pub const PROTOCOL_VERSION: u32 = 1;

    pub fn runtime_socket_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        std::path::Path::new(&base).join(SOCKET_RELATIVE_PATH)
    }

    pub fn config_file_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            format!("{home}/.config")
        });
        std::path::Path::new(&base)
            .join("evermore")
            .join("index-files.json")
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Scope {
        pub id: String,
        pub path: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SearchResult {
        pub scope_id: String,
        pub path: String,
        pub excerpt: String,
        pub score: f32,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        Health,
        ListScopes,
        Reindex { scope_id: String },
        RemoveScope { scope_id: String },
        Search { query: String, limit: Option<usize> },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "event", rename_all = "snake_case")]
    pub enum Event {
        Health {
            protocol_version: u32,
            ready: bool,
        },
        Scopes {
            scopes: Vec<Scope>,
        },
        Indexed {
            scope_id: String,
            documents: usize,
            chunks: usize,
        },
        Removed {
            scope_id: String,
        },
        Results {
            query: String,
            results: Vec<SearchResult>,
        },
        Error {
            message: String,
        },
    }
}

/// Read-only context requests for AI clients. The service applies its local
/// source allowlist before returning any context.
pub mod ai_context {
    use serde::{Deserialize, Serialize};

    pub const SOCKET_RELATIVE_PATH: &str = "evermore/ai-contextd.sock";
    pub const PROTOCOL_VERSION: u32 = 1;

    pub fn runtime_socket_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        std::path::Path::new(&base).join(SOCKET_RELATIVE_PATH)
    }

    pub fn config_file_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            format!("{home}/.config")
        });
        std::path::Path::new(&base)
            .join("evermore")
            .join("context-sources.json")
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        Health,
        ListSources,
        Get {
            source: String,
        },
        /// Internal compositor snapshot published by the Sway bridge. This
        /// updates observation cache only; it cannot execute compositor IPC.
        UpdateCompositor {
            data: serde_json::Value,
        },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "event", rename_all = "snake_case")]
    pub enum Event {
        Health {
            protocol_version: u32,
            ready: bool,
        },
        Sources {
            sources: Vec<String>,
        },
        Context {
            source: String,
            data: serde_json::Value,
        },
        Updated {
            source: String,
        },
        Error {
            message: String,
        },
    }
}

impl Response {
    pub fn ok(data: impl Serialize) -> Self {
        Response::Ok {
            data: serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Response::Err {
            message: message.into(),
        }
    }
}

/// Wire protocol for `evermore-settingsd` and its clients. Same framing
/// (newline-delimited JSON) and the same `Ok`/`Err` response shape as the
/// signer's, kept in its own module since it's a different socket/daemon.
pub mod settings {
    use serde::{Deserialize, Serialize};

    pub const SOCKET_RELATIVE_PATH: &str = "evermore/settingsd.sock";

    pub fn runtime_socket_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        std::path::Path::new(&base).join(SOCKET_RELATIVE_PATH)
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        /// List the settings domains currently known (i.e. that have at
        /// least one key set).
        ListDomains,
        /// Get a single key's value within a domain.
        Get { domain: String, key: String },
        /// List every key/value pair in a domain.
        GetDomain { domain: String },
        /// Set a key's value within a domain. Triggers a NIP-44-encrypted
        /// kind:30078 event (one per domain, replacing the previous one)
        /// to be signed and published to the control-plane relay.
        Set {
            domain: String,
            key: String,
            value: serde_json::Value,
        },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "status", rename_all = "snake_case")]
    pub enum Response {
        Ok { data: serde_json::Value },
        Err { message: String },
    }

    impl Response {
        pub fn ok(data: impl Serialize) -> Self {
            Response::Ok {
                data: serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
            }
        }

        pub fn err(message: impl Into<String>) -> Self {
            Response::Err {
                message: message.into(),
            }
        }
    }
}

/// Wire protocol for `evermore-notifyd` and its clients (the panel's
/// Notification Center). Same framing as the other two sockets.
pub mod notifications {
    use serde::{Deserialize, Serialize};

    pub const SOCKET_RELATIVE_PATH: &str = "evermore/notifyd.sock";

    pub fn runtime_socket_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        std::path::Path::new(&base).join(SOCKET_RELATIVE_PATH)
    }

    /// Where a notification came from -- shown in the panel so the merged
    /// timeline (PROJECT_IDEA.md's Phase 3 goal) can be filtered by source.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Source {
        /// A local-relay event tagged ["t","evermore-notify"] -- a daemon
        /// on this machine announcing something (e.g. an update available).
        Local,
        /// A public mention/reply (kind:1 with a `p` tag for us) from an
        /// external relay.
        Mention,
        /// A NIP-17 gift-wrapped DM, unwrapped.
        Dm,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct NotificationRecord {
        pub id: String,
        pub source: Source,
        pub title: String,
        pub body: String,
        pub received_at: u64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        /// Most-recent-first, capped at whatever the daemon keeps in memory.
        ListRecent,
        /// Remove all current notification metadata from the local in-memory view.
        ClearRecent,
        /// Dismiss one non-DM notification by its local identifier.
        Dismiss { notification_id: String },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "status", rename_all = "snake_case")]
    pub enum Response {
        Ok { data: serde_json::Value },
        Err { message: String },
    }

    impl Response {
        pub fn ok(data: impl Serialize) -> Self {
            Response::Ok {
                data: serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
            }
        }

        pub fn err(message: impl Into<String>) -> Self {
            Response::Err {
                message: message.into(),
            }
        }
    }
}
