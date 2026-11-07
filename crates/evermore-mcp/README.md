# evermore-mcp

An MCP stdio adapter over the shared operation catalogue, signer, and
`evermore-operationsd`. It exposes catalogue operations as MCP tools plus
`mcp_status` and `op_status`. The adapter has no shell, raw signing, approval,
or direct execution tool.

## Configuration

Start the adapter as the owning desktop user with a dedicated agent identity
already loaded in `evermore-signer`:

```sh
EVERMORE_MCP_AGENT_IDENTITY=assistant \
EVERMORE_MCP_OWNER_PUBKEY=<owner-pubkey-hex> \
evermore-mcp
```

The MCP host must launch this command as a stdio subprocess. The inherited
user session selects the local Unix sockets; the configured agent key binds
all tool calls from that subprocess to one Nostr identity. Do not configure
the owner's key as the agent identity. The owner public key is used only as
the recipient for encrypted write requests.

For a loopback Streamable HTTP endpoint, run the same adapter with `--http`:

```sh
EVERMORE_MCP_AGENT_IDENTITY=assistant \
EVERMORE_MCP_OWNER_PUBKEY=<owner-pubkey-hex> \
EVERMORE_MCP_HTTP_TOKEN=<random-secret-at-least-32-characters> \
EVERMORE_MCP_HTTP_BIND=127.0.0.1:8765 \
evermore-mcp --http
```

The endpoint is `http://127.0.0.1:8765/mcp`. The bind address must be a
loopback IP. Every request requires the bearer token and an allowed Host;
present Origin headers must exactly match a local HTTP origin. HTTP sessions
receive a cryptographically random `MCP-Session-Id` during initialization and
must send it, plus `MCP-Protocol-Version: 2025-11-25`, on later POST requests.
SSE GET and session deletion return 405. The bearer token is a local access
secret for the configured agent identity; it does not add operation grants.
Keep it private and do not expose this endpoint through a proxy or public
interface.

## Tools and authority

`tools/list` derives operation names, descriptions, input/output schemas,
grant labels, and read/write annotations from `evermore-common::operations`.
Every operation call requires an active owner-signed grant for the exact
operation name. Reads route through `evermore-operationsd`. Writes are NIP-44
encrypted to the owner, signed by the configured agent, then submitted to
`evermore-operationsd`; the daemon applies grant checks and owner approval
before any side effect.

`mcp_status` reports adapter reachability and whether the configured agent
identity resolves in the signer. It does not contact `evermore-operationsd`
or the relay. `op_status` returns a redacted operation record only when its
actor matches the configured agent identity. If `evermore-operationsd` is
stopped, `op_status` reads the private local journal without changing it and
applies the same actor check and redaction.

The stdio and Streamable HTTP transports follow MCP `2025-11-25`. HTTP returns
one JSON object per request and does not offer server-initiated SSE streams.
The MCP specification permits a server to reject GET with 405 when it does
not offer an SSE stream.
