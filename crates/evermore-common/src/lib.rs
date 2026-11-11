//! Wire protocol shared between `evermore-signer` and its clients.
//!
//! The socket speaks newline-delimited JSON: one [`Request`] per line in,
//! one [`Response`] per line out. Kept free of any `nostr`-crate types so
//! clients don't have to pull in signing/crypto code just to talk to the
//! daemon -- event payloads cross the wire as plain JSON.

use serde::{Deserialize, Serialize};

/// Base directory for every Evermore daemon's control socket.
///
/// Deliberately NOT `$XDG_RUNTIME_DIR`: each of these daemons runs as its
/// own `systemd --user` unit with `ProtectHome=read-only`, and `RuntimeDirectory=evermore`
/// (or no RuntimeDirectory= at all) under that sandboxing gives each UNIT
/// its own private, unit-scoped view of `$XDG_RUNTIME_DIR/evermore` --
/// invisible to every other unit, and to the unsandboxed session (sway,
/// evermore-panel) that is these daemons' actual client. Verified live: a
/// socket bound under `$XDG_RUNTIME_DIR/evermore/` from inside one unit's
/// sandbox cannot be opened by any other process, sandboxed or not, even
/// with `JoinsNamespaceOf=` declared; the same socket bound under
/// `$HOME/.local/share/evermore/` -- already a real, shared bind mount
/// every one of these units exposes via `ReadWritePaths=%h/.local/share/evermore`
/// -- is reachable from anywhere. Falls back to `/tmp` if neither
/// `XDG_DATA_HOME` nor `HOME` is set, mainly so this doesn't panic outside
/// a real session.
pub fn user_data_home() -> std::path::PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
}

/// Path (relative to [`user_data_home`]) of the signer's Unix socket.
pub const SOCKET_RELATIVE_PATH: &str = "evermore/signer.sock";

/// Resolves the signer socket path under [`user_data_home`].
pub fn runtime_socket_path() -> std::path::PathBuf {
    user_data_home().join(SOCKET_RELATIVE_PATH)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Request {
    /// List the identity names currently loaded in the daemon.
    ListIdentities,
    /// Generate a new agent identity named `agent-<slug>` and load it into
    /// the daemon's keyring. The secret is also written to the signer's
    /// private identity directory so it survives logout and is reloaded on
    /// the next start. Only identities in the `agent-` namespace may be
    /// created or deleted through this request.
    CreateAgentIdentity { slug: String },
    /// Delete an agent identity from the keyring and, if present, from the
    /// persisted identity directory. Refuses anything outside `agent-`.
    DeleteAgentIdentity { slug: String },
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
        crate::user_data_home().join(SOCKET_RELATIVE_PATH)
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        Catalog,
        ConfiguredOwner,
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

    /// Whether an operation is safe to expose through the MCP adapter.
    /// Read operations are available directly; writes are exposed only when
    /// the operations daemon has an owner-approval path for them.
    pub fn is_mcp_exposed(definition: &Definition) -> bool {
        definition.access == Access::Read || definition.approval == Approval::Required
    }

    /// Build the current catalogue in stable name order. Interfaces should
    /// consume these definitions rather than duplicating tool metadata.
    pub fn catalog() -> Vec<Definition> {
        let mut definitions = vec![
            definition(
                "calendar.create_event",
                "Create a timed NIP-52 calendar event. Public events publish in the clear; private events use recipient-specific NIP-59 gift wraps. Requires owner approval.",
                Access::Write,
                "calendar.write",
                Approval::Required,
                object_schema(
                    json!({
                        "title": { "type": "string", "minLength": 1, "maxLength": 256 },
                        "start": { "type": "integer", "minimum": 1, "description": "Inclusive Unix timestamp in seconds." },
                        "end": { "type": "integer", "minimum": 1, "description": "Exclusive Unix timestamp in seconds; must follow start." },
                        "timezone": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Installed IANA time zone, such as Australia/Melbourne." },
                        "description": { "type": "string", "maxLength": 8192 },
                        "calendar": { "type": "string", "maxLength": 256 },
                        "visibility": { "type": "string", "enum": ["public", "private"] },
                        "relay_url": { "type": "string", "minLength": 9, "maxLength": 2048, "description": "Owner-selected relay from EVERMORE_CALENDAR_RELAYS; shown in the approval review." },
                        "participants": { "type": "array", "maxItems": 64, "items": { "type": "string", "minLength": 64, "maxLength": 64 } }
                    }),
                    &[
                        "title",
                        "start",
                        "end",
                        "timezone",
                        "visibility",
                        "relay_url",
                    ],
                ),
                object_schema(
                    json!({ "event_id": { "type": "string" }, "coordinate": { "type": "string" }, "kind": { "type": "integer" }, "visibility": { "type": "string" }, "relay_url": { "type": "string" }, "published": { "type": "boolean" } }),
                    &[
                        "event_id",
                        "coordinate",
                        "kind",
                        "visibility",
                        "relay_url",
                        "published",
                    ],
                ),
                Redaction::None,
            ),
            definition(
                "calendar.update_event",
                "Edit an owner-authored NIP-52 event by publishing a new version at the same addressable coordinate. For kind 31923, provide Unix start/end and timezone; for kind 31922, provide date_start/date_end (exclusive). Private events are re-wrapped for their existing recipients. Requires owner approval.",
                Access::Write,
                "calendar.write",
                Approval::Required,
                object_schema(
                    json!({
                        "event_id": { "type": "string", "minLength": 1, "maxLength": 256 },
                        "expected_title": { "type": "string", "minLength": 1, "maxLength": 256, "description": "Current title shown in the approval review, to guard against editing a stale or mismatched event." },
                        "title": { "type": "string", "minLength": 1, "maxLength": 256 },
                        "start": { "type": "integer", "minimum": 1 },
                        "end": { "type": "integer", "minimum": 1 },
                        "timezone": { "type": "string", "minLength": 1, "maxLength": 128 },
                        "date_start": { "type": "string", "minLength": 10, "maxLength": 10, "description": "Inclusive YYYY-MM-DD for a kind-31922 event." },
                        "date_end": { "type": "string", "minLength": 10, "maxLength": 10, "description": "Optional exclusive YYYY-MM-DD for a kind-31922 event." },
                        "description": { "type": "string", "maxLength": 8192 },
                        "relay_url": { "type": "string", "minLength": 9, "maxLength": 2048 },
                        "participants": { "type": "array", "maxItems": 64, "items": { "type": "string", "minLength": 64, "maxLength": 64 } }
                    }),
                    &["event_id", "expected_title", "title", "relay_url"],
                ),
                object_schema(
                    json!({ "event_id": { "type": "string" }, "coordinate": { "type": "string" }, "kind": { "type": "integer" }, "visibility": { "type": "string" }, "relay_url": { "type": "string" }, "published": { "type": "boolean" } }),
                    &[
                        "event_id",
                        "coordinate",
                        "kind",
                        "visibility",
                        "relay_url",
                        "published",
                    ],
                ),
                Redaction::None,
            ),
            definition(
                "calendar.delete_event",
                "Delete an owner-authored NIP-52 event. Public events use a NIP-09 deletion request; private events send a recipient-specific encrypted withdrawal, which cannot revoke retained copies. Requires owner approval.",
                Access::Write,
                "calendar.write",
                Approval::Required,
                object_schema(
                    json!({
                        "event_id": { "type": "string", "minLength": 1, "maxLength": 256 },
                        "expected_title": { "type": "string", "minLength": 1, "maxLength": 256, "description": "Current title shown in the approval review, to guard against deleting a stale or mismatched event." },
                        "relay_url": { "type": "string", "minLength": 9, "maxLength": 2048 }
                    }),
                    &["event_id", "expected_title", "relay_url"],
                ),
                object_schema(
                    json!({ "event_id": { "type": "string" }, "coordinate": { "type": "string" }, "visibility": { "type": "string" }, "relay_url": { "type": "string" }, "deletion_requested": { "type": "boolean" }, "private_withdrawal_sent": { "type": "boolean" } }),
                    &[
                        "event_id",
                        "coordinate",
                        "visibility",
                        "relay_url",
                        "deletion_requested",
                        "private_withdrawal_sent",
                    ],
                ),
                Redaction::None,
            ),
            definition(
                "calendar.get",
                "Read one cached calendar event by its local event identifier.",
                Access::Read,
                "calendar.read",
                Approval::None,
                object_schema(
                    json!({ "event_id": { "type": "string", "minLength": 1, "maxLength": 128 } }),
                    &["event_id"],
                ),
                json!({ "type": "object" }),
                Redaction::SecretShapedValues,
            ),
            definition(
                "calendar.list",
                "List public NIP-52 calendar events and private events gift-wrapped to the owner and stored on the local control-plane relay. Optionally cap results; records include event timestamps and time zones.",
                Access::Read,
                "calendar.read",
                Approval::None,
                object_schema(
                    json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 100 } }),
                    &[],
                ),
                json!({ "type": "array", "items": { "type": "object" } }),
                Redaction::SecretShapedValues,
            ),
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
                "notifications.set_dnd",
                "Enable or disable SwayNotificationCenter Do Not Disturb. Approval decisions still require an explicit owner action. Requires owner approval.",
                Access::Write,
                "notifications.write",
                Approval::Required,
                object_schema(json!({ "enabled": { "type": "boolean" } }), &["enabled"]),
                object_schema(json!({ "dnd": { "type": "boolean" } }), &["dnd"]),
                Redaction::None,
            ),
            definition(
                "packages.install_pinned",
                "Propose installation or update to one exact package version from a trusted, signed npack release (publisher/name, as reported by packages.list_available). The archive hash must match before the current package version changes; a mismatch is removed again rather than left installed.",
                Access::Write,
                "packages.write",
                Approval::Required,
                object_schema(
                    json!({
                        "publisher": { "type": "string", "format": "hex_pubkey" },
                        "name": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" },
                        "version": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" },
                        "sha256": { "type": "string", "format": "sha256" }
                    }),
                    &["publisher", "name", "version", "sha256"],
                ),
                object_schema(
                    json!({
                        "publisher": { "type": "string" }, "name": { "type": "string" },
                        "version": { "type": "string" }, "sha256": { "type": "string" }
                    }),
                    &["publisher", "name", "version", "sha256"],
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
                "Discover the latest package versions from publishers in the owner's trusted-publisher list, via npack's catalogue. Use these values to explain updates; installation still requires an exact owner-approved proposal (packages.install_pinned needs publisher/name/version plus a hash, which this listing does not include -- resolve those immediately before proposing, not from a cached copy).",
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
                "packages.update_plan",
                "Check installed packages against the owner's trusted publishers, resolve the exact available version and SHA-256 for each update, and report resolver errors and revocations separately. Read-only; each install still requires its own owner approval.",
                Access::Read,
                "packages.read",
                Approval::None,
                object_schema(json!({}), &[]),
                object_schema(
                    json!({
                        "count": { "type": "integer", "minimum": 0 },
                        "updates": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "publisher": { "type": "string" },
                                    "name": { "type": "string" },
                                    "current_version": { "type": "string" },
                                    "available_version": { "type": "string" },
                                    "sha256": { "type": "string" },
                                    "reference": { "type": "string" }
                                },
                                "required": ["publisher", "name", "current_version", "available_version", "sha256", "reference"],
                                "additionalProperties": false
                            }
                        },
                        "revoked": { "type": "array", "items": { "type": "object" } },
                        "errors": { "type": "array", "items": { "type": "object" } }
                    }),
                    &["count", "updates", "revoked", "errors"],
                ),
                Redaction::None,
            ),
            definition(
                "packages.resolve",
                "Resolve one package's current trusted release (publisher/name, as reported by packages.list_available or packages.list_installed) to its exact version and SHA-256, immediately before proposing packages.install_pinned -- the catalogue listing alone doesn't carry a hash.",
                Access::Read,
                "packages.read",
                Approval::None,
                object_schema(
                    json!({
                        "publisher": { "type": "string", "format": "hex_pubkey" },
                        "name": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" }
                    }),
                    &["publisher", "name"],
                ),
                json!({ "type": "object" }),
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
                "Explain an installed package's declared direct dependencies and whether each is installed.",
                Access::Read,
                "packages.read",
                Approval::None,
                object_schema(
                    json!({
                        "publisher": { "type": "string", "format": "hex_pubkey" },
                        "name": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" }
                    }),
                    &["publisher", "name"],
                ),
                object_schema(
                    json!({
                        "publisher": { "type": "string" },
                        "name": { "type": "string" },
                        "version": { "type": "string" },
                        "dependencies": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "publisher": { "type": ["string", "null"] },
                                    "requirement": { "type": "string" },
                                    "installed": { "type": "boolean" },
                                    "installed_version": { "type": ["string", "null"] }
                                },
                                "required": ["name", "publisher", "requirement", "installed", "installed_version"],
                                "additionalProperties": false
                            }
                        }
                    }),
                    &["publisher", "name", "version", "dependencies"],
                ),
                Redaction::None,
            ),
            definition(
                "packages.remove",
                "Propose removal of one installed npack package (publisher/name) and its package-managed executable link. Requires owner approval.",
                Access::Write,
                "packages.write",
                Approval::Required,
                object_schema(
                    json!({
                        "publisher": { "type": "string", "format": "hex_pubkey" },
                        "name": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" }
                    }),
                    &["publisher", "name"],
                ),
                object_schema(
                    json!({ "publisher": { "type": "string" }, "name": { "type": "string" }, "removed": { "type": "boolean" } }),
                    &["publisher", "name", "removed"],
                ),
                Redaction::None,
            ),
            definition(
                "ai.model.install_signed_npack",
                "Import the declared GGUF (and optional LoRA adapter) from an already installed npack package (publisher/name, as reported by packages.list_installed) into the local model store. Requires owner approval.",
                Access::Write,
                "packages.write",
                Approval::Required,
                object_schema(
                    json!({
                        "publisher": { "type": "string", "format": "hex_pubkey" },
                        "name": { "type": "string", "minLength": 1, "maxLength": 128, "format": "path_component" }
                    }),
                    &["publisher", "name"],
                ),
                object_schema(
                    json!({ "publisher": { "type": "string" }, "name": { "type": "string" }, "model": { "type": "object" } }),
                    &["publisher", "name", "model"],
                ),
                Redaction::None,
            ),
            definition(
                "ai.infer.status",
                "Check whether Evermore's local inference service and its llama.cpp backend are available. This reports service readiness only; it does not load a model.",
                Access::Read,
                "ai.model.read",
                Approval::None,
                object_schema(json!({}), &[]),
                object_schema(
                    json!({
                        "protocol_version": { "type": "integer" },
                        "backend": { "type": "string" },
                        "ready": { "type": "boolean" }
                    }),
                    &["protocol_version", "backend", "ready"],
                ),
                Redaction::None,
            ),
            definition(
                "ai.model.list",
                "List locally installed chat and embedding models, including their capabilities, context size, adapter, and whether each model is already warm. This does not load or unload models.",
                Access::Read,
                "ai.model.read",
                Approval::None,
                object_schema(json!({}), &[]),
                json!({
                    "type": "array",
                    "items": { "type": "object" }
                }),
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
                "ai.context.status",
                "Check whether Evermore's context service is available and report its protocol version. This does not read context data.",
                Access::Read,
                "ai.context.read",
                Approval::None,
                object_schema(json!({}), &[]),
                object_schema(
                    json!({
                        "protocol_version": { "type": "integer" },
                        "ready": { "type": "boolean" }
                    }),
                    &["protocol_version", "ready"],
                ),
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
                    "source": { "type": "string", "enum": ["compositor", "settings:theme", "notifications:local", "notifications:mention", "packages:installed", "calendar:events"] }
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
                "ai.index.status",
                "Check whether Evermore's local file index is available and report how many user-selected files are indexed. File paths are returned separately by ai.index.list_scopes.",
                Access::Read,
                "ai.index.read",
                Approval::None,
                object_schema(json!({}), &[]),
                object_schema(
                    json!({
                        "protocol_version": { "type": "integer" },
                        "ready": { "type": "boolean" },
                        "selected_files": { "type": "integer" }
                    }),
                    &["protocol_version", "ready", "selected_files"],
                ),
                Redaction::None,
            ),
            definition(
                "ai.index.list_scopes",
                "List the file paths the owner explicitly selected for local AI indexing. Use these exact paths with ai.index.read. Paths are user-approved local scopes; do not infer or probe other files.",
                Access::Read,
                "ai.index.read",
                Approval::None,
                object_schema(json!({}), &[]),
                json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "scope_id": { "type": "string" },
                            "path": { "type": "string" }
                        },
                        "required": ["scope_id", "path"],
                        "additionalProperties": false
                    }
                }),
                Redaction::SecretShapedValues,
            ),
            definition(
                "ai.index.read",
                "Read one exact file already selected for AI indexing. The service enforces the active scope allowlist, file-size limit, supported file types, and no-AI markers.",
                Access::Read,
                "ai.index.read",
                Approval::None,
                object_schema(
                    json!({ "path": { "type": "string", "minLength": 1, "maxLength": 4096 } }),
                    &["path"],
                ),
                object_schema(
                    json!({
                        "source": { "type": "string" },
                        "data": { "type": "object" }
                    }),
                    &["source", "data"],
                ),
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
                "Set a setting value. For the desktop theme, use domain 'theme' and key 'mode' with value 'dark' or 'light'. Requires owner approval before execution.",
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

    /// Stable SHA-256 over the serialized catalogue. Used to bind a trained
    /// adapter and its evaluation data to the exact operation registry.
    pub fn catalog_fingerprint() -> String {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&catalog()).expect("operation catalogue serializes");
        hex::encode(Sha256::digest(&bytes))
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
            if schema.get("format").and_then(Value::as_str) == Some("hex_pubkey")
                && (text.len() != 64
                    || !text
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
            {
                return Err(format!(
                    "{path} must be a 64-character lowercase hexadecimal public key"
                ));
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

        #[test]
        fn calendar_mutations_are_approval_gated_and_review_bound() {
            for name in ["calendar.update_event", "calendar.delete_event"] {
                let definition = find(name).expect("calendar mutation in catalogue");
                assert_eq!(definition.access, Access::Write);
                assert_eq!(definition.approval, Approval::Required);
                assert_eq!(definition.required_grants, vec!["calendar.write"]);
            }

            let identifier = "a".repeat(64);
            let update = json!({
                "event_id": identifier.clone(),
                "expected_title": "Planning",
                "title": "Planning updated",
                "relay_url": "wss://calendar.example"
            });
            assert_eq!(
                canonicalize_input("calendar.update_event", &update).unwrap(),
                update
            );
            assert!(
                canonicalize_input(
                    "calendar.delete_event",
                    &json!({"event_id": identifier, "relay_url": "wss://calendar.example"})
                )
                .is_err()
            );

            let private_create = json!({
                "title": "Private planning",
                "start": 1_800_000_000_u64,
                "end": 1_800_003_600_u64,
                "timezone": "Australia/Melbourne",
                "visibility": "private",
                "relay_url": "wss://calendar.example",
                "participants": ["b".repeat(64)]
            });
            assert!(canonicalize_input("calendar.create_event", &private_create).is_ok());
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
        crate::user_data_home().join(INFER_SOCKET_RELATIVE_PATH)
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
            if let Some(redacted) = redact_secret_assignments(line) {
                output.push_str(&redacted);
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

    fn redact_secret_assignments(line: &str) -> Option<String> {
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

        let mut output = String::new();
        let mut cursor = 0;
        let mut copied_until = 0;
        let mut changed = false;

        while cursor < line.len() {
            let Some((offset, _)) = line[cursor..]
                .char_indices()
                .find(|(_, ch)| ch.is_ascii_alphanumeric() || *ch == '_')
            else {
                break;
            };
            let key_start = cursor + offset;
            let mut key_end = key_start;
            while key_end < line.len() {
                let ch = line[key_end..].chars().next().unwrap();
                if !ch.is_ascii_alphanumeric() && ch != '_' {
                    break;
                }
                key_end += ch.len_utf8();
            }
            let key = &line[key_start..key_end];
            if !SECRET_LABELS
                .iter()
                .any(|label| key.eq_ignore_ascii_case(label))
            {
                cursor = key_end;
                continue;
            }

            let mut separator = key_end;
            while separator < line.len() {
                let ch = line[separator..].chars().next().unwrap();
                if !ch.is_whitespace() || ch == '\n' || ch == '\r' {
                    break;
                }
                separator += ch.len_utf8();
            }
            while separator < line.len()
                && matches!(line.as_bytes()[separator], b'"' | b'\'' | b'`')
            {
                separator += 1;
            }
            if separator >= line.len() || !matches!(line.as_bytes()[separator], b'=' | b':') {
                cursor = key_end;
                continue;
            }
            separator += 1;
            while separator < line.len() {
                let ch = line[separator..].chars().next().unwrap();
                if !ch.is_whitespace() || ch == '\n' || ch == '\r' {
                    break;
                }
                separator += ch.len_utf8();
            }
            if separator >= line.len() || line.as_bytes()[separator] == b'\n' {
                cursor = key_end;
                continue;
            }

            let first = line[separator..].chars().next().unwrap();
            let (value_start, value_end) = if matches!(first, '"' | '\'') {
                let value_start = separator + first.len_utf8();
                let mut value_end = value_start;
                let mut escaped = false;
                let mut closing = None;
                for (offset, ch) in line[value_start..].char_indices() {
                    if escaped {
                        escaped = false;
                    } else if ch == '\\' {
                        escaped = true;
                    } else if ch == first {
                        closing = Some(value_start + offset);
                        break;
                    }
                    value_end = value_start + offset + ch.len_utf8();
                }
                (value_start, closing.unwrap_or(value_end))
            } else {
                let value_start = separator;
                let value_end = line[value_start..]
                    .char_indices()
                    .find(|(_, ch)| matches!(ch, ',' | ';' | '&' | '\n' | '\r'))
                    .map(|(offset, _)| value_start + offset)
                    .unwrap_or(line.len());
                (value_start, value_end)
            };

            if value_end <= value_start {
                cursor = key_end;
                continue;
            }
            output.push_str(&line[copied_until..value_start]);
            output.push_str("[REDACTED]");
            cursor = value_end;
            copied_until = value_end;
            changed = true;
        }

        if changed {
            output.push_str(&line[copied_until..]);
            Some(output)
        } else {
            None
        }
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
            if token.chars().any(char::is_alphanumeric) {
                *redact_next = false;
                return "[REDACTED]".into();
            }
            return token.to_string();
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

    /// An `ai_model` object embedded in an npack package's own
    /// `.npack/manifest.json`, by convention rather than a field npack's
    /// own `Manifest` type knows about -- npack's deserializer (no
    /// `deny_unknown_fields`) drops unrecognised keys when it parses that
    /// file into its typed struct, but the raw bytes npack retains (and
    /// publishes/verifies) still carry it, so this is read directly from
    /// those bytes, never through npack's own manifest type.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct AiModelPayload {
        /// Base GGUF file, relative to the install prefix (e.g.
        /// `share/evermore/models/<name>/base.gguf`), the same way any
        /// other payload file in the package is.
        pub gguf_file: String,
        pub sha256: String,
        #[serde(default)]
        pub adapter_gguf_file: Option<String>,
        #[serde(default)]
        pub adapter_sha256: Option<String>,
        #[serde(default)]
        pub adapter_scale: Option<f32>,
        #[serde(default)]
        pub context_size: Option<u32>,
        #[serde(default)]
        pub quant: Option<String>,
        #[serde(default)]
        pub capabilities: Vec<String>,
        #[serde(default)]
        pub operation_registry_version: Option<String>,
        #[serde(default)]
        pub training: Option<TrainingProvenance>,
    }

    /// Provenance for a trained LoRA adapter. Build tooling writes it into the
    /// package; the store records it so an adapter is never detached from the
    /// data, base model, and catalogue it was evaluated against.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TrainingProvenance {
        pub base_model_sha256: String,
        pub dataset_sha256: String,
        pub config_sha256: String,
        #[serde(default)]
        pub eval_report_sha256: Option<String>,
        pub catalogue_fingerprint: String,
        #[serde(default)]
        pub contributed_traces: bool,
    }

    impl TrainingProvenance {
        fn validate(&self) -> Result<(), String> {
            for (label, value) in [
                ("base-model", &self.base_model_sha256),
                ("dataset", &self.dataset_sha256),
                ("training-config", &self.config_sha256),
                ("catalogue-fingerprint", &self.catalogue_fingerprint),
            ] {
                if !is_sha256(value) {
                    return Err(format!("training {label} SHA-256 is malformed"));
                }
            }
            if let Some(report) = &self.eval_report_sha256
                && !is_sha256(report)
            {
                return Err("training evaluation-report SHA-256 is malformed".into());
            }
            Ok(())
        }
    }

    fn is_sha256(value: &str) -> bool {
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
        pub training: Option<TrainingProvenance>,
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
            if let Some(training) = &self.training {
                training.validate()?;
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
            // Deliberately NOT /var/lib/evermore/models: that directory is
            // already actively managed as a 0700, root-only secrets store by
            // evermore-pkcs11 and evermore-signer's device-identity
            // bootstrap, both of which reset it to 0700 unconditionally on
            // every run. A world-readable subdirectory nested inside it
            // would get silently re-locked the next time either of those
            // happened to run, leaving every non-root reader (evermore-
            // inferd included) permission-denied on the parent.
            PathBuf::from("/var/lib/evermore-models"),
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
        /// True only when this model is already warm in the local process
        /// pool. Reading this state must never load a model.
        #[serde(default)]
        pub loaded: bool,
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
            /// Refuse the request unless the selected model is already running.
            /// Used by latency-bounded launcher suggestions.
            #[serde(default)]
            warm_only: bool,
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
            publisher: String,
            name: String,
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
                training: None,
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
        fn training_provenance_is_validated() {
            let mut value = manifest(true);
            value.training = Some(TrainingProvenance {
                base_model_sha256: "a".repeat(64),
                dataset_sha256: "b".repeat(64),
                config_sha256: "c".repeat(64),
                eval_report_sha256: Some("d".repeat(64)),
                catalogue_fingerprint: "e".repeat(64),
                contributed_traces: true,
            });
            value.validate().unwrap();
            value.training.as_mut().unwrap().dataset_sha256 = "short".into();
            assert!(value.validate().unwrap_err().contains("dataset"));
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
        fn redacts_spaced_and_quoted_secret_assignments() {
            assert_eq!(
                redact_text("password = p8-semantic-secret\n"),
                "password = [REDACTED]\n"
            );
            assert_eq!(
                redact_text("{\"api_key\": \"two word secret\", \"name\": \"safe\"}\n"),
                "{\"api_key\": \"[REDACTED]\", \"name\": \"safe\"}\n"
            );
            assert_eq!(
                redact_text("password: \"two word secret\"\n"),
                "password: \"[REDACTED]\"\n"
            );
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
                    grammar,
                    backend,
                    warm_only,
                    ..
                } => {
                    assert!(grammar.is_none());
                    assert_eq!(backend, Backend::Auto);
                    assert!(!warm_only);
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
        crate::user_data_home().join(SOCKET_RELATIVE_PATH)
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
        crate::user_data_home().join(SOCKET_RELATIVE_PATH)
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
        /// Reads one exact path only when ai-indexd currently lists it as an
        /// explicitly selected, regular, non-`no-ai` file scope.
        GetIndexedFile {
            path: String,
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
        crate::user_data_home().join(SOCKET_RELATIVE_PATH)
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
        crate::user_data_home().join(SOCKET_RELATIVE_PATH)
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
        /// Enable or disable SwayNotificationCenter Do Not Disturb.
        SetDnd { enabled: bool },
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

/// Task and recipe protocol for the deterministic agent runner
/// (`evermore-agentd`). Tasks group existing catalogue operations; a task is
/// never execution authority and never introduces a second operation
/// lifecycle.
pub mod agent {
    use crate::operations::{self, Access, Approval, Definition};
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    pub const SOCKET_RELATIVE_PATH: &str = "evermore/agentd.sock";
    pub const PROTOCOL_VERSION: u32 = 1;
    pub const TASK_TAG: &str = "task";
    pub const STEP_TAG: &str = "step";
    pub const TASKS_FILE: &str = "evermore/tasks/tasks.json";
    pub const ROUTINES_FILE: &str = "evermore/routines/routines.json";
    pub const AGENTS_FILE: &str = "evermore/agents/agents.json";
    pub const AGENT_RUNTIME_FILE: &str = "evermore/agents/runtime.json";
    pub const MAX_LOCAL_RECIPE_BYTES: u64 = 256 * 1024;
    pub const MAX_RECIPE_STEPS: u32 = 32;
    pub const MAX_RECIPE_SECONDS: u64 = 300;
    pub const PROPOSAL_MARKER: &str = "EVERMORE_PROPOSAL:";
    pub const PROPOSAL_CONTEXT_TOKENS: u32 = 4096;

    /// Agent definitions live on the control-plane relay as owner-authored
    /// NIP-78 events. The `d` tag is `evermore:agent:<id>`; a NIP-51 list at
    /// `evermore:agents` records which agent pubkeys are currently enrolled.
    pub const AGENT_PREFIX: &str = "agent-";
    pub const AGENT_D_TAG_PREFIX: &str = "evermore:agent:";
    pub const AGENTS_LIST_D_TAG: &str = "evermore:agents";
    pub const KIND_AGENT_DEFINITION: u16 = 30078;
    pub const KIND_AGENTS_LIST: u16 = 30000;
    pub const AGENT_DEFINITION_VERSION: u32 = 1;

    /// The user-facing agent id. It is the `d`-tag suffix and the suffix of
    /// the signer identity (`agent-<slug>`); keep it filesystem- and
    /// systemd-instance-safe.
    pub fn validate_agent_slug(slug: &str) -> Result<(), String> {
        if slug.is_empty() {
            return Err("agent slug must not be empty".to_string());
        }
        if slug.len() > 48 {
            return Err("agent slug must be at most 48 characters".to_string());
        }
        if slug.starts_with(AGENT_PREFIX) {
            return Err("agent slug must not include the 'agent-' prefix".to_string());
        }
        if !slug.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        }) {
            return Err(
                "agent slug may only contain lowercase ASCII letters, digits, '-' or '_'"
                    .to_string(),
            );
        }
        let first = slug.as_bytes()[0];
        let last = slug.as_bytes()[slug.len() - 1];
        if !(first.is_ascii_alphanumeric() && last.is_ascii_alphanumeric()) {
            return Err("agent slug must start and end with a letter or digit".to_string());
        }
        Ok(())
    }

    pub fn agent_identity_name(slug: &str) -> Result<String, String> {
        validate_agent_slug(slug)?;
        Ok(format!("{AGENT_PREFIX}{slug}"))
    }

    pub fn agent_d_tag(slug: &str) -> Result<String, String> {
        validate_agent_slug(slug)?;
        Ok(format!("{AGENT_D_TAG_PREFIX}{slug}"))
    }

    pub fn agents_file_path() -> Result<PathBuf, String> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())?;
        Ok(base.join(AGENTS_FILE))
    }

    pub fn agent_runtime_file_path() -> Result<PathBuf, String> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())?;
        Ok(base.join(AGENT_RUNTIME_FILE))
    }

    pub fn runtime_socket_path() -> PathBuf {
        crate::user_data_home().join(SOCKET_RELATIVE_PATH)
    }

    pub fn tasks_file_path() -> Result<PathBuf, String> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())?;
        Ok(base.join(TASKS_FILE))
    }

    pub fn routines_file_path() -> Result<PathBuf, String> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())?;
        Ok(base.join(ROUTINES_FILE))
    }

    pub fn recipes_dir() -> Result<PathBuf, String> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())?;
        Ok(base.join("evermore/recipes"))
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "method", content = "params", rename_all = "snake_case")]
    pub enum Request {
        Health,
        ConfiguredAgent,
        ListRecipes,
        RunRecipe {
            recipe_id: String,
            #[serde(default)]
            inputs: Value,
        },
        /// Generate one validated approval-gated write proposal from a natural
        /// language request. This never submits an operation request.
        ProposeOperation {
            prompt: String,
        },
        /// Run one bounded local MCP tool conversation as the configured agent.
        McpRequest {
            prompt: String,
        },
        TaskStatus {
            task_id: String,
        },
        ListTasks {
            limit: usize,
        },
        CancelTask {
            task_id: String,
        },
        /// Return the cached, relay-projected agent definitions.
        ListAgents,
        /// Return the curated starting points for the Agents gallery.
        ListAgentTemplates,
        /// Re-read owner-authored agent definitions from the control-plane
        /// relay, replacing the local cache. Returns the projected agents and
        /// any per-definition warnings.
        ReloadAgents,
        /// Run one agent job immediately, without changing its schedule.
        RunAgentJob {
            agent_id: String,
            job_id: String,
        },
        ListRoutines,
        AddRoutine {
            id: String,
            recipe_id: String,
            #[serde(default)]
            inputs: Value,
            interval_seconds: u64,
            agent_identity: String,
            #[serde(default = "default_enabled")]
            enabled: bool,
        },
        SetRoutineEnabled {
            routine_id: String,
            enabled: bool,
        },
        RemoveRoutine {
            routine_id: String,
        },
    }

    fn default_enabled() -> bool {
        true
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Routine {
        pub id: String,
        pub recipe_id: String,
        #[serde(default)]
        pub inputs: Value,
        pub interval_seconds: u64,
        /// Signer identity this routine runs as. It must be distinct from the
        /// interactive assistant and the owner.
        pub agent_identity: String,
        pub enabled: bool,
        #[serde(default)]
        pub last_run_at: Option<u64>,
        #[serde(default)]
        pub last_task_id: Option<String>,
        #[serde(default)]
        pub last_error: Option<String>,
        pub created_at: u64,
    }

    /// One scheduled recipe inside an agent definition. An agent can have
    /// several independent jobs, each with its own cadence.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct AgentJob {
        pub id: String,
        pub recipe_id: String,
        #[serde(default)]
        pub inputs: Value,
        pub interval_seconds: u64,
        #[serde(default = "default_enabled")]
        pub enabled: bool,
    }

    /// The owner-authored definition of one automated agent. This is the
    /// decrypted JSON content of the relay's `kind:30078` event; the `d` tag
    /// carries `id` and is not part of the payload.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct AgentDefinition {
        #[serde(default = "default_agent_definition_version")]
        pub version: u32,
        pub id: String,
        pub title: String,
        #[serde(default)]
        pub description: String,
        /// Signer identity name; always `agent-<id>`.
        pub identity: String,
        /// Hex public key of that identity, checked against the signer.
        pub agent_pubkey: String,
        #[serde(default = "default_enabled")]
        pub enabled: bool,
        pub jobs: Vec<AgentJob>,
        #[serde(default)]
        pub created_at: u64,
        #[serde(default)]
        pub updated_at: u64,
    }

    impl AgentDefinition {
        pub fn validate(&self) -> Result<(), String> {
            if self.version != AGENT_DEFINITION_VERSION {
                return Err(format!(
                    "agent definition version {} is not supported (expected {AGENT_DEFINITION_VERSION})",
                    self.version
                ));
            }
            validate_agent_slug(&self.id)?;
            let expected = agent_identity_name(&self.id)?;
            if self.identity != expected {
                return Err(format!(
                    "agent '{}' must use identity '{expected}', not '{}'",
                    self.id, self.identity
                ));
            }
            if self.agent_pubkey.len() != 64
                || !self.agent_pubkey.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(format!("agent '{}' has an invalid agent_pubkey", self.id));
            }
            if self.title.trim().is_empty() || self.title.len() > 128 {
                return Err(format!(
                    "agent '{}' title must be 1-128 characters",
                    self.id
                ));
            }
            if self.description.len() > 4096 {
                return Err(format!("agent '{}' description is too long", self.id));
            }
            if self.jobs.is_empty() {
                return Err(format!("agent '{}' must have at least one job", self.id));
            }
            let mut seen = std::collections::BTreeSet::new();
            for job in &self.jobs {
                if job.id.is_empty()
                    || job.id.len() > 64
                    || !job.id.bytes().all(|b| {
                        b.is_ascii_lowercase()
                            || b.is_ascii_digit()
                            || b == b'.'
                            || b == b'-'
                            || b == b'_'
                    })
                {
                    return Err(format!(
                        "agent '{}' has an invalid job id '{}'",
                        self.id, job.id
                    ));
                }
                if !seen.insert(job.id.clone()) {
                    return Err(format!("agent '{}' repeats job id '{}'", self.id, job.id));
                }
                if job.recipe_id.trim().is_empty() {
                    return Err(format!(
                        "agent '{}' job '{}' has no recipe",
                        self.id, job.id
                    ));
                }
                if job.interval_seconds == 0 || job.interval_seconds > 365 * 24 * 60 * 60 {
                    return Err(format!(
                        "agent '{}' job '{}' interval must be between 1 second and 365 days",
                        self.id, job.id
                    ));
                }
            }
            Ok(())
        }
    }

    /// Local execution state for one job. This is never part of the relay
    /// definition; it follows the machine, not the identity.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct AgentJobRuntime {
        #[serde(default)]
        pub last_run_at: Option<u64>,
        #[serde(default)]
        pub last_task_id: Option<String>,
        #[serde(default)]
        pub last_error: Option<String>,
    }

    /// A curated starting point shown in the Agents gallery. The recipe is
    /// the actual nscript behaviour; the manifest only supplies presentation
    /// metadata and defaults.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct AgentTemplate {
        pub id: String,
        #[serde(rename = "recipe")]
        pub recipe_id: String,
        pub name: String,
        pub summary: String,
        #[serde(default)]
        pub category: String,
        pub cadence_seconds: u64,
        pub expiry_days: u64,
        #[serde(default)]
        pub badge: String,
        /// Manual templates create a disabled schedule; Run now still works.
        #[serde(default)]
        pub manual: bool,
        #[serde(default)]
        pub requires: Vec<String>,
    }

    /// A cached definition plus whether its signing identity is available on
    /// this device. A relay definition synced from another machine can exist
    /// without its key; the UI shows it as unavailable instead of dropping it.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct AgentView {
        #[serde(flatten)]
        pub definition: AgentDefinition,
        pub available: bool,
        #[serde(default)]
        pub unavailable_reason: Option<String>,
        /// Runtime state keyed by job id.
        #[serde(default)]
        pub runtime: BTreeMap<String, AgentJobRuntime>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct AgentReload {
        pub agents: Vec<AgentView>,
        #[serde(default)]
        pub warnings: Vec<String>,
    }

    fn default_agent_definition_version() -> u32 {
        AGENT_DEFINITION_VERSION
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
    pub enum TaskStatus {
        Pending,
        Running,
        AwaitingApproval,
        Succeeded,
        Failed,
        Denied,
        Expired,
        Cancelled,
        PartiallyCompleted,
        ExecutionUnknown,
        Replanned,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum StepStatus {
        Pending,
        Running,
        Succeeded,
        Failed,
        Denied,
        Expired,
        Cancelled,
        AwaitingApproval,
        ExecutionUnknown,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum StepKind {
        Select,
        Read,
        Journal,
        Render,
        Write,
        Each,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TaskBudget {
        pub steps: u32,
        pub writes: u32,
        pub seconds: u64,
        /// Maximum model-context tokens a recipe may use. Recipes with no model
        /// step ignore it; M11.2's L1 generation enforces the same cap directly.
        #[serde(default = "default_context_tokens")]
        pub context_tokens: u32,
    }

    fn default_context_tokens() -> u32 {
        4096
    }

    impl Default for TaskBudget {
        fn default() -> Self {
            Self {
                steps: 8,
                writes: 0,
                seconds: 30,
                context_tokens: default_context_tokens(),
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TaskStep {
        pub index: u32,
        pub id: String,
        pub kind: StepKind,
        pub operation: Option<String>,
        #[serde(default)]
        pub arguments: Option<Value>,
        #[serde(default)]
        pub operation_id: Option<String>,
        pub status: StepStatus,
        #[serde(default)]
        pub result: Option<Value>,
        #[serde(default)]
        pub error: Option<String>,
        #[serde(default)]
        pub started_at: Option<u64>,
        #[serde(default)]
        pub finished_at: Option<u64>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Task {
        pub task_id: String,
        /// Set when the task was created by a standing routine.
        #[serde(default)]
        pub routine_id: Option<String>,
        /// Set when the task was created by a relay-defined agent job.
        #[serde(default)]
        pub agent_id: Option<String>,
        #[serde(default)]
        pub job_id: Option<String>,
        pub recipe_id: String,
        pub recipe_version: u32,
        /// `builtin` or `local`; local recipes are unsigned and recorded so the
        /// audit trail can distinguish them.
        pub recipe_source: String,
        pub recipe_sha256: String,
        pub agent_pubkey: String,
        pub owner_pubkey: String,
        pub status: TaskStatus,
        #[serde(default)]
        pub inputs: Value,
        pub budget: TaskBudget,
        pub steps: Vec<TaskStep>,
        #[serde(default)]
        pub cancel_requested: bool,
        #[serde(default)]
        pub result: Option<Value>,
        #[serde(default)]
        pub error: Option<String>,
        #[serde(default)]
        pub status_source: Option<String>,
        pub created_at: u64,
        pub updated_at: u64,
        pub expires_at: u64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct RecipeSummary {
        pub id: String,
        pub version: u32,
        pub title: String,
        pub description: String,
        pub source: String,
        pub operations: Vec<String>,
        pub budget: TaskBudget,
        /// Object schema for the recipe's `select` inputs, generated from the
        /// recipe's declared slots. The panel renders a simple form from it.
        pub inputs_schema: Value,
    }

    /// The one-operation shape emitted by the panel's proposal marker and by
    /// L1 generation.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ProposalDraft {
        pub operation: String,
        pub arguments: Value,
    }

    /// A validated L1 proposal. It has not been submitted; the owner still
    /// approves the resulting operation request.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Proposal {
        pub operation: String,
        pub arguments: Value,
        pub model: String,
        pub attempts: u32,
        pub prompt_tokens: Option<u64>,
        pub completion_tokens: Option<u64>,
        pub context_budget: u32,
        /// The request text shown to the model, surfaced next to the proposal
        /// so a steered or injected proposal is visible at review time.
        pub triggering_prompt: String,
    }

    /// Build the system instructions for one catalogue-constrained write
    /// proposal. Returns `None` when no write operation matches the request.
    pub fn write_proposal_instructions(prompt: &str, catalog: &[Definition]) -> Option<String> {
        write_proposal_prompt(prompt, catalog, false)
    }

    /// Constrained variant used by L1 generation, where the model output is
    /// grammar-limited to one proposal or a refusal object.
    pub fn write_constrained_proposal_instructions(
        prompt: &str,
        catalog: &[Definition],
    ) -> Option<String> {
        write_proposal_prompt(prompt, catalog, true)
    }

    fn write_proposal_prompt(
        prompt: &str,
        catalog: &[Definition],
        constrained: bool,
    ) -> Option<String> {
        let terms = prompt_terms(prompt);
        if terms.is_empty() {
            return None;
        }
        let mut relevant: Vec<(usize, &Definition)> = catalog
            .iter()
            .filter(|definition| {
                definition.access == Access::Write && definition.approval == Approval::Required
            })
            .filter_map(|definition| {
                let searchable = format!("{} {}", definition.name, definition.description);
                let score = prompt_terms(&searchable).intersection(&terms).count();
                (score > 0).then_some((score, definition))
            })
            .collect();
        relevant.sort_by(|(score_a, a), (score_b, b)| {
            score_b.cmp(score_a).then_with(|| a.name.cmp(&b.name))
        });
        relevant.truncate(5);
        if relevant.is_empty() {
            return None;
        }
        let catalogue: Vec<_> = relevant
            .into_iter()
            .map(|(_, definition)| {
                serde_json::json!({
                    "operation": definition.name,
                    "description": definition.description,
                    "arguments_schema": definition.input_schema,
                })
            })
            .collect();
        let ending = if constrained {
            "Otherwise only emit {\"refusal\":\"short reason\"}."
        } else {
            "Otherwise do not emit the marker."
        };
        Some(format!(
            "You may suggest a system change only when the user clearly requests one. Use only the approval-gated write operations in this catalogue, and provide arguments that match the schema. A suggestion is not an action: do not claim it was submitted, approved, or executed. If a change is clearly requested and its arguments are known, finish with one exact marker line containing JSON in this shape: {} {{\"operation\":\"catalogue.name\",\"arguments\":{{}}}}. {} The UI will validate the proposal and the owner must still approve it. Relevant operation catalogue: {}",
            PROPOSAL_MARKER,
            ending,
            serde_json::to_string(&catalogue).unwrap_or_else(|_| "[]".into())
        ))
    }

    /// Parse the last proposal marker in model output. A refusal object is
    /// reported as an error so the caller can show it without treating it as a
    /// proposal.
    pub fn parse_proposal_marker(content: &str) -> Result<Option<ProposalDraft>, String> {
        if let Some(line) = content
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with(PROPOSAL_MARKER))
        {
            let json = line
                .trim_start()
                .strip_prefix(PROPOSAL_MARKER)
                .unwrap_or_default()
                .trim();
            return serde_json::from_str(json)
                .map(Some)
                .map_err(|error| format!("invalid operation proposal JSON: {error}"));
        }
        let trimmed = content.trim();
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            if let Some(reason) = value.get("refusal").and_then(Value::as_str) {
                return Err(format!("Model declined: {reason}"));
            }
        }
        Ok(None)
    }

    /// Validate a draft against the live catalogue and return its canonical
    /// arguments. Only approval-gated write operations are accepted.
    pub fn validate_write_proposal(draft: &ProposalDraft) -> Result<(Definition, Value), String> {
        let definition = operations::find(&draft.operation)
            .ok_or_else(|| "operation is not in the current catalogue".to_string())?;
        if definition.access != Access::Write || definition.approval != Approval::Required {
            return Err("only approval-gated write operations can be proposed".into());
        }
        let arguments = operations::canonicalize_input(&draft.operation, &draft.arguments)?;
        Ok((definition, arguments))
    }

    fn prompt_terms(text: &str) -> std::collections::HashSet<String> {
        const STOP_WORDS: &[&str] = &[
            "about", "after", "also", "and", "are", "for", "from", "help", "into", "make",
            "please", "that", "the", "this", "with", "would",
        ];
        text.split(|character: char| !character.is_alphanumeric())
            .map(str::to_ascii_lowercase)
            .filter(|term| term.chars().count() > 2 && !STOP_WORDS.contains(&term.as_str()))
            .collect()
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct AgentHealth {
        pub protocol_version: u32,
        pub agent_pubkey: String,
        pub owner_pubkey: String,
    }

    /// Tag pair marking a request event as a step of a task.
    pub fn task_tag(task_id: &str) -> Vec<String> {
        vec![TASK_TAG.to_string(), task_id.to_string()]
    }

    /// Tag pair carrying a task step's zero-based index.
    pub fn step_tag(index: u32) -> Vec<String> {
        vec![STEP_TAG.to_string(), index.to_string()]
    }

    /// Read the private task store without requiring the agent daemon. Running
    /// tasks are reported as `execution_unknown` and are never replayed.
    pub fn read_local_tasks(limit: usize) -> Result<Vec<Task>, String> {
        let path = tasks_file_path()?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            format!("couldn't read local task store {}: {error}", path.display())
        })?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("local task store must be a regular file with private permissions".into());
        }
        let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
        let file: TaskFile = serde_json::from_slice(&bytes)
            .map_err(|error| format!("couldn't parse local task store: {error}"))?;
        let now = unix_time();
        let mut tasks: Vec<Task> = file
            .tasks
            .into_values()
            .map(|task| normalize_offline_task(task, now))
            .collect();
        tasks.sort_by_key(|task| std::cmp::Reverse(task.created_at));
        tasks.truncate(limit.clamp(1, 100));
        Ok(tasks)
    }

    fn normalize_offline_task(mut task: Task, now: u64) -> Task {
        let interrupted = task.status == TaskStatus::Running
            || task
                .steps
                .iter()
                .any(|step| step.status == StepStatus::Running);
        if interrupted {
            task.status = TaskStatus::ExecutionUnknown;
            task.error = Some(
                "agent daemon is unavailable after a step was claimed; outcome requires review and must not be replayed".into(),
            );
            for step in &mut task.steps {
                if step.status == StepStatus::Running {
                    step.status = StepStatus::ExecutionUnknown;
                    step.error = Some(
                        "step was claimed when the agent daemon stopped; outcome requires review"
                            .into(),
                    );
                }
            }
        } else if task.status == TaskStatus::Pending && now >= task.expires_at {
            task.status = TaskStatus::Expired;
        }
        task.status_source = Some("local_store_offline".into());
        task
    }

    #[derive(Default, Serialize, Deserialize)]
    struct TaskFile {
        #[serde(default)]
        tasks: BTreeMap<String, Task>,
    }

    fn unix_time() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn task_and_step_tags_are_stable() {
            assert_eq!(task_tag("abc"), vec!["task".to_string(), "abc".to_string()]);
            assert_eq!(step_tag(2), vec!["step".to_string(), "2".to_string()]);
        }

        #[test]
        fn task_status_round_trips() {
            let status = TaskStatus::PartiallyCompleted;
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, "\"partially_completed\"");
            assert_eq!(
                serde_json::from_str::<TaskStatus>(&json).unwrap(),
                TaskStatus::PartiallyCompleted
            );
            assert_eq!(
                serde_json::from_str::<TaskStatus>("\"future_state\"").unwrap(),
                TaskStatus::Unknown
            );
        }

        #[test]
        fn proposal_marker_parses_and_validates() {
            let draft = parse_proposal_marker(
                "Preparing.\nEVERMORE_PROPOSAL: {\"operation\":\"settings.set\",\"arguments\":{\"domain\":\"theme\",\"key\":\"mode\",\"value\":\"dark\"}}",
            )
            .unwrap()
            .unwrap();
            assert_eq!(draft.operation, "settings.set");
            let (_, arguments) = validate_write_proposal(&draft).unwrap();
            assert_eq!(arguments["domain"], "theme");
        }

        #[test]
        fn proposal_refusal_and_read_operations_are_rejected() {
            let refusal = parse_proposal_marker("{\"refusal\":\"no matching write\"}").unwrap_err();
            assert!(refusal.contains("no matching write"));

            let read = ProposalDraft {
                operation: "settings.get".into(),
                arguments: serde_json::json!({"domain": "theme", "key": "mode"}),
            };
            assert!(validate_write_proposal(&read).is_err());
        }

        #[test]
        fn proposal_instructions_include_matching_writes() {
            let catalog = operations::catalog();
            let instructions =
                write_proposal_instructions("change the theme to dark", &catalog).unwrap();
            assert!(instructions.contains("settings.set"));
            assert!(instructions.contains(PROPOSAL_MARKER));
            assert!(!instructions.contains("\"refusal\""));
            let constrained =
                write_constrained_proposal_instructions("change the theme to dark", &catalog)
                    .unwrap();
            assert!(constrained.contains("\"refusal\""));
        }

        fn valid_agent() -> AgentDefinition {
            AgentDefinition {
                version: AGENT_DEFINITION_VERSION,
                id: "morning-digest".into(),
                title: "Morning digest".into(),
                description: String::new(),
                identity: "agent-morning-digest".into(),
                agent_pubkey: "a".repeat(64),
                enabled: true,
                jobs: vec![AgentJob {
                    id: "digest".into(),
                    recipe_id: "audit.digest".into(),
                    inputs: serde_json::json!({}),
                    interval_seconds: 86_400,
                    enabled: true,
                }],
                created_at: 0,
                updated_at: 0,
            }
        }

        #[test]
        fn agent_definition_rejects_mismatched_identity_and_bad_jobs() {
            let mut agent = valid_agent();
            assert!(agent.validate().is_ok());
            assert!(
                agent_d_tag(&agent.id)
                    .unwrap()
                    .starts_with(AGENT_D_TAG_PREFIX)
            );

            agent.identity = "agent-other".into();
            assert!(agent.validate().is_err());

            let mut agent = valid_agent();
            agent.jobs[0].interval_seconds = 0;
            assert!(agent.validate().is_err());

            let mut agent = valid_agent();
            agent.jobs.push(agent.jobs[0].clone());
            assert!(agent.validate().is_err());

            let mut agent = valid_agent();
            agent.jobs[0].id = "Digest".into();
            assert!(agent.validate().is_err());
        }

        #[test]
        fn agent_slugs_are_filesystem_and_systemd_safe() {
            for slug in ["morning-digest", "a", "package_watch_2", "x1"] {
                assert!(validate_agent_slug(slug).is_ok(), "{slug}");
            }
            for slug in [
                "",
                "Morning-Digest",
                "agent-morning",
                "../escape",
                "with/slash",
                ".hidden",
                "-leading",
                "trailing-",
                "with space",
            ] {
                assert!(validate_agent_slug(slug).is_err(), "{slug}");
            }
        }
    }
}
