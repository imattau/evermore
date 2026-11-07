# evermore-contextd

Read-only context broker for local AI clients. It listens on
`$XDG_RUNTIME_DIR/evermore/ai-contextd.sock` and uses the newline-delimited
`evermore_common::ai_context` protocol.

Set `EVERMORE_CONTEXT_SOURCES` to a JSON array to opt in to sources, for
example `["compositor","settings:theme","notifications:mention"]`. When
unset, selections come from `$XDG_CONFIG_HOME/evermore/context-sources.json`
or `~/.config/evermore/context-sources.json`; the panel manages this file.
The supported sources are focused Sway window/workspace metadata, the theme
settings domain, local or mention notification metadata, and installed NPK
package manifests. Direct messages are never exposed. Sensitive-looking
strings are redacted before context is returned. With no allowlist configured,
the service returns no context.

The daemon only reads existing service sockets, Sway state, and verified
installed package manifests. It has no operation-execution or settings-write
path.
