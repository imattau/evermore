# evermore-sway-ai-bridge

Polls Sway's read-only IPC tree for the focused window and workspace, then
publishes changes to the private `ai-contextd` socket. The context daemon
sanitizes the snapshot and returns it only if the user enabled the `compositor`
context source. The bridge cannot issue Sway commands or perform operations.

The user service needs the Sway session environment (`SWAYSOCK` and
`XDG_RUNTIME_DIR`) and `evermore-contextd` running. If either service is
unavailable, the bridge retries and reports the failure to stderr.
