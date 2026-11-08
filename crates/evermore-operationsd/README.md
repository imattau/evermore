# evermore-operationsd

Local operation catalogue and approval-gated operation executor.

The daemon listens on `$XDG_RUNTIME_DIR/evermore/operationsd.sock` with
owner-only directory/socket permissions. The service uses the configured
owner identity only to verify approval events. A request's actor is taken from
the verified Nostr signature, never from a client-supplied actor field.

## Start

Load the owner identity and a distinct service identity into
`evermore-signer`, then run:

```sh
evermore-signer keygen personal-operations
evermore-operationsd serve --owner-identity personal --service-identity personal-operations
```

The owner and service identities must be distinct. The service identity signs
execution results. The user unit is `evermore-operationsd@personal.service`
and uses `personal-operations` for the service identity. It starts the
matching `evermore-settingsd@personal`, signer, and local relay units as
dependencies.

## Read operations

```sh
evermore-operationsd catalog
evermore-operationsd invoke settings.list_domains '{}'
evermore-operationsd invoke settings.get '{"domain":"theme","key":"mode"}'
```

Read calls route to the existing daemons. Arguments are validated against the
shared catalogue and model-visible results pass through its redaction policy.
`packages.*` talks to `npackd` -- https://github.com/imattau/npack's own
package-manager daemon over its JSON-RPC Unix socket, not a reimplementation
in this repo -- identifying packages by `publisher`/`name` rather than a
single id. `packages.list_installed` lists what's installed;
`packages.list_available` browses the catalogue of the owner's trusted
publishers, which does not carry a SHA-256; `packages.resolve` gets one
package's exact current version and hash immediately before proposing
`packages.install_pinned`, which still binds to that exact version and
SHA-256 and requires owner approval (a mismatch between what npack actually
installs and the approved hash is removed again, not left installed).
`ai.model.install_signed_npack` also takes `publisher`/`name`, same as the
rest, but is otherwise unrelated: it imports a GGUF/LoRA model from an
`ai_model` object a publisher embeds in their package's own
`.npack/manifest.json` -- a field `npack`'s own manifest type has no
equivalent for and silently drops, which is fine, since `evermore-inferd`
reads it directly from the installed package's retained, verified archive
rather than through `npack`'s typed manifest.

## Approval-gated writes

```sh
evermore-operationsd request settings.set \
  '{"domain":"theme","key":"mode","value":"dark"}' \
  --identity assistant --owner-pubkey <owner-pubkey-hex>
evermore-operationsd approve <operation-id> approve --identity personal
evermore-operationsd status <operation-id>
```

`status` normally reads the live daemon. If the daemon is stopped, it falls
back to the permission-restricted local journal and marks the response with
`status_source: local_journal_offline`. Offline reads report expired pending
requests as expired and conservatively report a persisted `running` claim as
`execution_unknown`; they never execute or mutate the journal.

Owner-signed grants let an agent submit only the named write operations:

```sh
evermore-operationsd grant <agent-pubkey-hex> settings.set --identity personal
evermore-operationsd grant <agent-pubkey-hex> settings.set --identity personal --expires-in 86400
evermore-operationsd grants
evermore-operationsd revoke-grant <grant-event-id> --identity personal
```

The operation list is comma-separated when granting more than one operation.
Grants expire after 30 days by default and cannot exceed 365 days. The owner
signs each grant and revocation. A grant must be acknowledged by the local
relay before it authorizes requests. Revocation takes effect in the local
store immediately, blocks pending or approved requests from that agent, and
its relay publication retries if the relay is unavailable. The daemon checks
the grant when a request arrives, when approval is recorded, and immediately
before execution. A revoked grant does not cancel an operation that has
already started executing.

The request and approval are signed kind:1 events with Evermore-specific `t`
tags and are stored with the operation record. Request contents are NIP-44
encrypted to the owner before they are signed and published; the result event
contains only the operation ID and outcome status. The operation ID is the
request event ID. The owner approval references that ID in both its `e` tag
and content. Request and approval events must receive local relay acknowledgments
before the write can execute. The executor persists its claim before calling
`evermore-settingsd`. If the daemon restarts after claiming execution, it
records `execution_unknown` and does not replay the write.
If the settings daemon returns an error or its reply is lost after dispatch,
the outcome is also marked `execution_unknown` because the write may already
have taken effect.

The current executor supports catalogue-backed settings, notification,
package, model-import, and Sway operations. Package installation is pinned to a
trusted signed release and exact archive digest. Package removal and signed
model import also require owner approval; model import reads an installed
package and does not download it. Request details and results are kept in a
permission-restricted local state file, while the signed event chain is
published to the control-plane relay. Relay publication is retried from
durable state. Agent write requests require an active owner-signed grant for
that exact operation. Reads and writes exposed through MCP use this same
executor and catalogue.

## Optional encrypted activity export

Export is off unless the owner explicitly configures
`EVERMORE_ACTIVITY_EXPORT_RELAY` with a `ws://` or `wss://` relay URL. A manual
export requires both the service signing identity and owner identity:

```sh
EVERMORE_ACTIVITY_EXPORT_RELAY=wss://relay.example \
  evermore-operationsd export --identity personal-operations --owner-identity personal
```

The command reads the full local operation and grant journal, splits the JSON
into UTF-8-safe chunks below the NIP-44 plaintext limit, encrypts each chunk to
the owner's public key, and signs kind:1 export events with the service
identity. Each part is tagged with the SHA-256 export ID and its part number;
the command requires a positive relay acknowledgement for every part. It does
not run on a timer or send data anywhere unless the relay is configured and
the owner invokes the command. Owners can query the relay for the export
marker, group by export ID, sort parts by their `part` tags, decrypt each event
with the service pubkey as sender, then concatenate the plaintext chunks.
