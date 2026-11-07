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
        vec![
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
                            "source": { "type": "string", "enum": ["local", "mention", "dm"] },
                            "title": { "type": "string" },
                            "body": { "type": "string" },
                            "received_at": { "type": "integer", "minimum": 0 }
                        },
                        "required": ["source", "title", "body", "received_at"],
                        "additionalProperties": false
                    }
                }),
                Redaction::SecretShapedValues,
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
        ]
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

    /// Sanitize model-visible data according to the operation's declared
    /// policy. The human-facing UI can separately request the raw local data.
    pub fn redact_result(definition: &Definition, mut value: Value) -> Value {
        if definition.result_redaction == Redaction::SecretShapedValues {
            redact_value(&mut value, None);
        }
        value
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
