# evermore-operationsd

Local operation catalogue and approval-gated settings write path.

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

## Approval-gated writes

```sh
evermore-operationsd request settings.set \
  '{"domain":"theme","key":"mode","value":"dark"}' \
  --identity assistant --owner-pubkey <owner-pubkey-hex>
evermore-operationsd approve <operation-id> approve --identity personal
evermore-operationsd status <operation-id>
```

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

At this stage only `settings.set` has an executor. Request details and results
are kept in a permission-restricted local state file, while the signed event
chain is published to the control-plane relay. Relay publication is retried
from durable state. Agent write requests require an active owner-signed grant
for that exact operation. Read MCP identity authorization and broader policy
remain future work.
