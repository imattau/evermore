#!/usr/bin/env bash
# Exercise Evermore's local Qwen MCP adapter through agentd and the real MCP
# stdio server. Uses a disposable XDG tree and a preinstalled VM Qwen base.
set -euo pipefail

if [[ "${EVERMORE_PHASE11_KEYRING_SESSION:-}" != "1" ]]; then
    command -v keyctl >/dev/null || { echo "keyctl is required for the signer test" >&2; exit 1; }
    exec keyctl session evermore-phase11-test env EVERMORE_PHASE11_KEYRING_SESSION=1 bash "$0" "$@"
fi

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${EVERMORE_PHASE11_BIN_DIR:-$REPO_ROOT/target/debug}"
MODEL_ROOT="${EVERMORE_PHASE11_MODEL_ROOT:-$HOME/.cache/evermore-task-adapters}"
TEST_HOME="$(mktemp -d /tmp/evermore-phase11-mcp.XXXXXX)"
export HOME="$TEST_HOME"
export XDG_RUNTIME_DIR="$TEST_HOME/runtime" XDG_CONFIG_HOME="$TEST_HOME/config"
export XDG_DATA_HOME="$TEST_HOME/data" XDG_STATE_HOME="$TEST_HOME/state" XDG_CACHE_HOME="$TEST_HOME/cache"
export EVERMORE_MODEL_DIRS="$MODEL_ROOT" EVERMORE_MODEL_IMPORT_DIR="$MODEL_ROOT"
export EVERMORE_LLAMA_SERVER_BIN=/usr/bin/llama-server EVERMORE_INFER_MAX_MODELS=1
export EVERMORE_INFER_READY_TIMEOUT_SECS=240
export PATH="$BIN_DIR:$PATH"
export EVERMORE_MCP_COMMAND="$BIN_DIR/evermore-mcp"
export EVERMORE_MCP_MODEL="evermore/p8.model.qwen2.5-1.5b.mcp:1.0.0-dev.1"
export EVERMORE_PANEL_MODEL="evermore/p8.model.qwen2.5-1.5b.panel:1.0.0-dev.1"
RUN_ID="$(date +%s)-$$"
OWNER_ID="00-phase11-owner-$RUN_ID" AGENT_ID="10-phase11-agent-$RUN_ID"
PIDS=()

cleanup() {
    for pid in "${PIDS[@]}"; do kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true; done
    for pid in "${PIDS[@]}"; do wait "$pid" 2>/dev/null || true; done
    rm -rf -- "$TEST_HOME"
}
trap cleanup EXIT INT TERM

for binary in evermore-signer evermore-agentd evermore-mcp evermore-inferd; do
    [[ -x "$BIN_DIR/$binary" ]] || { echo "missing binary: $BIN_DIR/$binary" >&2; exit 1; }
done
for task in mcp panel; do
    model="$MODEL_ROOT/$task"
    [[ -f "$model/manifest.json" && -f "$model/adapter.gguf" && -f "$model/base.gguf" ]] || {
        echo "missing $task model adapter in $model" >&2; exit 1;
    }
    [[ "$(sha256sum "$model/base.gguf" | cut -d' ' -f1)" == "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["base"]["sha256"])' "$model/manifest.json")" ]] || {
        echo "$task model base hash mismatch" >&2; exit 1;
    }
done
mkdir -m 0700 -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

OWNER_PUBKEY="$("$BIN_DIR/evermore-signer" keygen "$OWNER_ID" | awk '$1 == "pubkey:" { print $2 }')"
"$BIN_DIR/evermore-signer" keygen "$AGENT_ID" >/dev/null
setsid "$BIN_DIR/evermore-signer" serve >"$TEST_HOME/signer.log" 2>&1 & PIDS+=("$!")
for _ in $(seq 1 50); do [[ -S "$XDG_RUNTIME_DIR/evermore/signer.sock" ]] && break; sleep .1; done
[[ -S "$XDG_RUNTIME_DIR/evermore/signer.sock" ]] || { cat "$TEST_HOME/signer.log" >&2; exit 1; }
setsid "$BIN_DIR/evermore-inferd" >"$TEST_HOME/inferd.log" 2>&1 & PIDS+=("$!")
for _ in $(seq 1 50); do [[ -S "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" ]] && break; sleep .1; done
[[ -S "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" ]] || { cat "$TEST_HOME/inferd.log" >&2; exit 1; }
setsid "$BIN_DIR/evermore-agentd" serve --agent-identity "$AGENT_ID" --owner-pubkey "$OWNER_PUBKEY" >"$TEST_HOME/agentd.log" 2>&1 & PIDS+=("$!")
for _ in $(seq 1 50); do [[ -S "$XDG_RUNTIME_DIR/evermore/agentd.sock" ]] && break; sleep .1; done
[[ -S "$XDG_RUNTIME_DIR/evermore/agentd.sock" ]] || { cat "$TEST_HOME/agentd.log" >&2; exit 1; }

PANEL_OUTPUT="$("$BIN_DIR/evermore-agentd" propose "Set desktop theme setting domain 'theme' key 'mode' to dark." 2>"$TEST_HOME/panel.log")" || {
    cat "$TEST_HOME/panel.log" "$TEST_HOME/inferd.log" "$TEST_HOME/agentd.log" >&2
    exit 1
}
python3 - "$PANEL_OUTPUT" <<'PY'
import json, sys
obj=json.loads(sys.argv[1])
assert obj.get("model")=="evermore/p8.model.qwen2.5-1.5b.panel:1.0.0-dev.1", obj
assert obj.get("operation")=="settings.set", obj
assert obj.get("arguments")=={"domain":"theme","key":"mode","value":"dark"}, obj
print("PANEL_ADAPTER_ROUTING_AND_VALIDATED_PROPOSAL_OK", json.dumps(obj, sort_keys=True))
PY

OUTPUT="$("$BIN_DIR/evermore-agentd" ask "Check whether Evermore MCP is reachable and report the configured agent identity." 2>"$TEST_HOME/ask.log")" || {
    cat "$TEST_HOME/ask.log" "$TEST_HOME/inferd.log" "$TEST_HOME/agentd.log" >&2
    exit 1
}
python3 - "$OUTPUT" "$AGENT_ID" <<'PY'
import json, sys
obj=json.loads(sys.argv[1])
assert obj.get("status")=="completed", obj
assert "reachable" in obj.get("answer", "").lower() or "identity" in obj.get("answer", "").lower(), obj
assert obj.get("tool_calls", 0) >= 1, obj
print("LOCAL_MCP_ADAPTER_AND_LIVE_STDIO_CALL_OK", json.dumps(obj, sort_keys=True))
PY
