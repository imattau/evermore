#!/usr/bin/env bash
# Validate the local semantic-search path with network egress disabled.
# Requires the signed BGE embedding package used by Phase 7.5 in the VM.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INFERD_BIN="${EVERMORE_P8_INFERD_BIN:-$REPO_ROOT/target/debug/evermore-inferd}"
INDEXD_BIN="${EVERMORE_P8_INDEXD_BIN:-$REPO_ROOT/target/debug/evermore-indexd}"
SOURCE_MODELS="${EVERMORE_P8_MODEL_DIR:-$HOME/.local/share/evermore/models}"
MODEL="${EVERMORE_P8_EMBED_MODEL:-evermore/p7.model.bge:1.0.0}"
LLAMA_SERVER="${EVERMORE_LLAMA_SERVER_BIN:-/usr/bin/llama-server}"

command -v bwrap >/dev/null || { echo "bubblewrap is required in the VM" >&2; exit 1; }
command -v ip >/dev/null || { echo "iproute2 is required in the VM" >&2; exit 1; }
[[ -x "$INFERD_BIN" ]] || { echo "missing inference daemon: $INFERD_BIN" >&2; exit 1; }
[[ -x "$INDEXD_BIN" ]] || { echo "missing index daemon: $INDEXD_BIN" >&2; exit 1; }
[[ -x "$LLAMA_SERVER" ]] || { echo "missing llama-server: $LLAMA_SERVER" >&2; exit 1; }
[[ -d "$SOURCE_MODELS" ]] || { echo "missing local model store: $SOURCE_MODELS" >&2; exit 1; }
sudo -n true || { echo "passwordless sudo is required for the private namespace" >&2; exit 1; }

WORKDIR="$(mktemp -d /tmp/evermore-p8-semantic.XXXXXX)"
cleanup() { sudo -n rm -rf -- "$WORKDIR"; }
trap cleanup EXIT
cp -a "$SOURCE_MODELS" "$WORKDIR/models"

cat >"$WORKDIR/inside.sh" <<'INNER'
#!/usr/bin/env bash
set -euo pipefail
WORKDIR="$1"
INFERD_BIN="$2"
INDEXD_BIN="$3"
MODEL="$4"
LLAMA_SERVER="$5"

ip link set lo up
export HOME="$WORKDIR/home"
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_RUNTIME_DIR="$WORKDIR/runtime"
export XDG_STATE_HOME="$WORKDIR/state"
export EVERMORE_INDEX_STATE_DIR="$WORKDIR/state/evermore-indexd"
export EVERMORE_MODEL_DIRS="$WORKDIR/models"
export EVERMORE_MODEL_IMPORT_DIR="$WORKDIR/models"
export EVERMORE_EMBED_MODEL="$MODEL"
export EVERMORE_LLAMA_SERVER_BIN="$LLAMA_SERVER"
export EVERMORE_INFER_READY_TIMEOUT_SECS=180
export EVERMORE_INDEX_FILES="[\"$HOME/notes.md\"]"

mkdir -p "$HOME/.config/evermore" "$XDG_RUNTIME_DIR" "$EVERMORE_INDEX_STATE_DIR"
chmod 700 "$HOME" "$HOME/.config" "$HOME/.config/evermore" "$XDG_RUNTIME_DIR" "$EVERMORE_INDEX_STATE_DIR"
cat >"$HOME/notes.md" <<'NOTES'
# Winter equipment storage

During the winter shutdown, store the community astronomy telescope in the
north equipment shed. Keep the reflector tube on padded shelf B, away from
roof leaks and the entrance door. The tripod can stay in the dry cabinet.

password = p8-semantic-secret-should-be-redacted
NOTES

"$INFERD_BIN" >"$WORKDIR/inferd.log" 2>&1 &
INFERD_PID=$!
"$INDEXD_BIN" >"$WORKDIR/indexd.log" 2>&1 &
INDEXD_PID=$!
stop_services() {
  children="$(pgrep -P "$INFERD_PID" 2>/dev/null || true)"
  kill "$INDEXD_PID" "$INFERD_PID" 2>/dev/null || true
  wait "$INDEXD_PID" 2>/dev/null || true
  wait "$INFERD_PID" 2>/dev/null || true
  if [[ -n "$children" ]]; then
    # shellcheck disable=SC2086
    kill $children 2>/dev/null || true
  fi
}
trap stop_services EXIT

for _ in $(seq 1 60); do
  [[ -S "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" && -S "$XDG_RUNTIME_DIR/evermore/ai-indexd.sock" ]] && break
  sleep 1
done
[[ -S "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" ]] || { cat "$WORKDIR/inferd.log" >&2; exit 1; }
[[ -S "$XDG_RUNTIME_DIR/evermore/ai-indexd.sock" ]] || { cat "$WORKDIR/indexd.log" >&2; exit 1; }

/usr/bin/python3 - "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" "$XDG_RUNTIME_DIR/evermore/ai-indexd.sock" "$MODEL" "$HOME/notes.md" <<'PY'
import json
import socket
import statistics
import sys
import time
from pathlib import Path

infer_socket, index_socket, model, source = sys.argv[1:]

def request(path, payload, timeout=120):
    client = socket.socket(socket.AF_UNIX)
    client.settimeout(timeout)
    client.connect(path)
    client.sendall(json.dumps(payload).encode() + bytes([10]))
    response = client.makefile().readline()
    client.close()
    if not response:
        raise AssertionError("service closed the socket without a response")
    return json.loads(response)

models = request(infer_socket, {"method": "list_models"}).get("models", [])
selected = next((item for item in models if item.get("name") == model), None)
assert selected and "embeddings" in selected.get("capabilities", []), models
print("EMBEDDING_MODEL_OK", model)

health = request(index_socket, {"method": "health"})
assert health == {"event": "health", "protocol_version": 1, "ready": True}, health
scopes = request(index_socket, {"method": "list_scopes"}).get("scopes", [])
assert len(scopes) == 1 and scopes[0]["path"] == str(Path(source).resolve()), scopes
indexed = request(index_socket, {"method": "reindex", "params": {"scope_id": scopes[0]["id"]}})
assert indexed.get("event") == "indexed" and indexed.get("chunks", 0) > 0, indexed
print("INDEXED_SELECTED_SCOPE_OK", "chunks=" + str(indexed["chunks"]))

payload = {"method": "search", "params": {
    "query": "Where should the astronomy telescope be stored during the winter?",
    "limit": 5,
}}
first = request(index_socket, payload)
assert first.get("event") == "results" and first.get("results"), first
top = first["results"][0]
assert top["path"] == str(Path(source).resolve()), top
assert "north equipment shed" in top["excerpt"].lower(), top
assert "p8-semantic-secret-should-be-redacted" not in top["excerpt"], top
assert "[REDACTED]" in top["excerpt"], top
print("SEMANTIC_RESULT_OK", "score=" + str(round(top["score"], 4)), "secret_redacted=yes")

samples = []
for _ in range(5):
    start = time.perf_counter_ns()
    response = request(index_socket, payload)
    samples.append((time.perf_counter_ns() - start) / 1_000_000)
    assert response.get("event") == "results" and response.get("results"), response
median = statistics.median(samples)
print("SEMANTIC_SEARCH_LATENCY_MS", "min=%.2f" % min(samples), "median=%.2f" % median, "max=%.2f" % max(samples))
if median <= 50:
    print("SEMANTIC_SEARCH_50MS_TARGET_OK")
else:
    print("SEMANTIC_SEARCH_50MS_TARGET_MISSED")

marker = Path(source + ".no-ai")
marker.write_text("owner opted out\n")
filtered = request(index_socket, {"method": "list_scopes"})
assert filtered.get("event") == "scopes" and not filtered.get("scopes"), filtered
print("NO_AI_SCOPE_FILTER_OK")
PY
INNER

sudo -n bwrap \
  --unshare-net \
  --cap-add CAP_NET_ADMIN \
  --ro-bind / / \
  --bind "$WORKDIR" "$WORKDIR" \
  --dev-bind /dev /dev \
  --proc /proc \
  /bin/bash "$WORKDIR/inside.sh" "$WORKDIR" "$INFERD_BIN" "$INDEXD_BIN" "$MODEL" "$LLAMA_SERVER"
