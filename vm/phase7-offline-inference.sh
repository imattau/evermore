#!/usr/bin/env bash
# Run signed local models and evermore-inferd in a private network namespace,
# allow loopback only, and prove an external connection is blocked.
#
# The model store must contain manifest.json + GGUF artifacts for each model
# (written by the signed npack import path). Set EVERMORE_P7_MODEL_DIR when the
# store is not the default per-user location.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INFERD_BIN="${EVERMORE_P7_INFERD_BIN:-$REPO_ROOT/target/debug/evermore-inferd}"
SOURCE_MODELS="${EVERMORE_P7_MODEL_DIR:-$HOME/.local/share/evermore/models}"
MODEL="${EVERMORE_P7_MODEL:-evermore/p7.model.bge:1.0.0}"
CHAT_MODEL="${EVERMORE_P7_CHAT_MODEL:-}"
LLAMA_SERVER="${EVERMORE_LLAMA_SERVER_BIN:-/usr/bin/llama-server}"

command -v bwrap >/dev/null || { echo "bubblewrap is required in the VM" >&2; exit 1; }
command -v ip >/dev/null || { echo "iproute2 is required in the VM" >&2; exit 1; }
[[ -x "$INFERD_BIN" ]] || { echo "missing inference daemon: $INFERD_BIN" >&2; exit 1; }
[[ -x "$LLAMA_SERVER" ]] || { echo "missing llama-server: $LLAMA_SERVER" >&2; exit 1; }
[[ -d "$SOURCE_MODELS" ]] || { echo "missing local model store: $SOURCE_MODELS" >&2; exit 1; }
sudo -n true || { echo "passwordless sudo is required for the private namespace" >&2; exit 1; }

WORKDIR="$(mktemp -d /tmp/evermore-p7-offline.XXXXXX)"
cleanup() { sudo -n rm -rf -- "$WORKDIR"; }
trap cleanup EXIT
cp -a "$SOURCE_MODELS" "$WORKDIR/models"
# An invalid manifest must be skipped without hiding the valid models.
mkdir -p "$WORKDIR/models/.broken"
printf '{"manifest_version":1}' >"$WORKDIR/models/.broken/manifest.json"

cat >"$WORKDIR/inside.sh" <<'INNER'
#!/usr/bin/env bash
set -euo pipefail
WORKDIR="$1"
INFERD_BIN="$2"
MODEL="$3"
CHAT_MODEL="$4"
LLAMA_SERVER="$5"

ip link set lo up
export HOME="$WORKDIR/home"
export XDG_RUNTIME_DIR="$WORKDIR/runtime"
export EVERMORE_MODEL_DIRS="$WORKDIR/models"
export EVERMORE_MODEL_IMPORT_DIR="$WORKDIR/models"
export EVERMORE_LLAMA_SERVER_BIN="$LLAMA_SERVER"
export EVERMORE_INFER_LOG_DIR="$WORKDIR/logs"
export EVERMORE_INFER_READY_TIMEOUT_SECS=180
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" "$WORKDIR/logs"

"$INFERD_BIN" >"$WORKDIR/inferd.log" 2>&1 &
INFERD_PID=$!
stop_services() {
  # Capture direct children before stopping the daemon; llama-server is spawned
  # by inferd and would otherwise be reparented and left running.
  children="$(pgrep -P "$INFERD_PID" 2>/dev/null || true)"
  kill "$INFERD_PID" 2>/dev/null || true
  wait "$INFERD_PID" 2>/dev/null || true
  if [[ -n "$children" ]]; then
    # shellcheck disable=SC2086
    kill $children 2>/dev/null || true
  fi
}
trap stop_services EXIT

for _ in $(seq 1 30); do
  [[ -S "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" ]] && break
  sleep 1
done
[[ -S "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" ]] || {
  echo "inference daemon failed to create its socket" >&2
  cat "$WORKDIR/inferd.log" >&2
  exit 1
}

/usr/bin/python3 - "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" "$MODEL" "$CHAT_MODEL" <<'PY'
import json
import socket
import sys
import time

def request(path, payload, timeout=90):
    client = socket.socket(socket.AF_UNIX)
    client.settimeout(timeout)
    client.connect(path)
    client.sendall(json.dumps(payload).encode() + bytes([10]))
    return client

def one_shot(path, payload, timeout=90):
    client = request(path, payload, timeout)
    event = json.loads(client.makefile().readline())
    client.close()
    return event

path = sys.argv[1]

# The store scan is the discovery path: no daemon-side model registry.
client = request(path, {"method": "list_models"})
models = json.loads(client.makefile().readline()).get("models", [])
client.close()
names = {model["name"]: model for model in models}
required = [sys.argv[2]] + ([sys.argv[3]] if sys.argv[3] else [])
missing = [model for model in required if model not in names]
if missing:
    raise SystemExit("models missing from local store: " + ", ".join(missing))
embedding = names[sys.argv[2]]
if "embeddings" not in embedding.get("capabilities", []):
    raise SystemExit("selected embedding model does not declare the embeddings capability")
print("MODEL_STORE_OK", "models=" + str(len(models)))

client = request(path, {
    "method": "embed",
    "params": {
        "request_id": "phase7-offline-validation",
        "model": sys.argv[2],
        "inputs": ["Local inference with external networking disabled"],
    },
})
response = json.loads(client.makefile().readline())
client.close()
assert response.get("event") == "embeddings", response
assert response.get("request_id") == "phase7-offline-validation", response
assert response.get("vectors") and response["vectors"][0], response
print(
    "LOCAL_EMBEDDING_OK",
    "dimensions=" + str(len(response["vectors"][0])),
    "request_id=" + response["request_id"],
)

if sys.argv[3]:
    def streamed_chat(request_id):
        started = time.perf_counter()
        client = request(path, {
            "method": "chat",
            "params": {
                "request_id": request_id,
                "model": sys.argv[3],
                "messages": [{"role": "user", "content": "Reply with a short greeting."}],
                "backend": "cpu",
                "max_tokens": 32,
                "temperature": 0.1,
            },
        }, timeout=180)
        stream = client.makefile()
        tokens = []
        first_token_ms = None
        completed = False
        while True:
            line = stream.readline()
            if not line:
                break
            event = json.loads(line)
            if event.get("request_id") != request_id:
                raise AssertionError(event)
            if event.get("event") == "token":
                if first_token_ms is None:
                    first_token_ms = (time.perf_counter() - started) * 1000
                tokens.append(event.get("text", ""))
            elif event.get("event") == "completed":
                completed = True
                break
            elif event.get("event") == "error":
                raise AssertionError(event)
        total_ms = (time.perf_counter() - started) * 1000
        client.close()
        assert completed and tokens, "chat stream did not produce tokens and complete"
        return first_token_ms, total_ms, tokens

    cold_ms, _, tokens = streamed_chat("phase7-offline-chat")
    print("LOCAL_STREAMED_CHAT_OK", "tokens=" + str(len(tokens)), "text=" + "".join(tokens))
    warm_ms, total_ms, _ = streamed_chat("phase8-warm-first-token")
    print("CHAT_COLD_FIRST_TOKEN_MS", "%.1f" % cold_ms)
    print("CHAT_WARM_FIRST_TOKEN_MS", "%.1f" % warm_ms, "total_ms=%.1f" % total_ms)
    print("CHAT_300MS_FIRST_TOKEN_TARGET_" + ("MET" if warm_ms <= 300 else "MISSED"))
    print("CHAT_3S_RESPONSE_TARGET_" + ("MET" if total_ms <= 3000 else "MISSED"))

    # GBNF must force exactly one operation-registry-shaped proposal.
    request_id = "phase7-grammar-chat"
    grammar = 'root ::= "{\\"operation\\":\\"settings.get\\",\\"arguments\\":{}}"'
    client = request(path, {
        "method": "chat",
        "params": {
            "request_id": request_id,
            "model": sys.argv[3],
            "messages": [{"role": "user", "content": "Emit the operation request."}],
            "backend": "cpu",
            "max_tokens": 32,
            "temperature": 0,
            "grammar": grammar,
        },
    }, timeout=180)
    stream = client.makefile()
    tokens = []
    while True:
        line = stream.readline()
        if not line:
            raise AssertionError("grammar chat stream ended early")
        event = json.loads(line)
        if event.get("request_id") != request_id:
            raise AssertionError(event)
        if event.get("event") == "token":
            tokens.append(event.get("text", ""))
        elif event.get("event") == "completed":
            break
        elif event.get("event") == "error":
            raise AssertionError(event)
    client.close()
    content = "".join(tokens)
    proposal = json.loads(content)
    assert proposal == {"operation": "settings.get", "arguments": {}}, content
    print("GRAMMAR_CHAT_OK", content)

adapters = one_shot(path, {"method": "list_adapters", "params": {"model": sys.argv[2]}})
assert adapters.get("event") == "adapters", adapters
assert adapters.get("adapters") == [], adapters
print("NO_ADAPTER_OK")

refused = one_shot(
    path, {"method": "set_adapter_scale", "params": {"model": sys.argv[2], "scale": 0.5}}
)
assert refused.get("event") == "error", refused
assert "no signed LoRA adapter" in refused.get("message", ""), refused
print("ADAPTER_SCALE_REFUSED_OK")

external = socket.socket()
external.settimeout(3)
try:
    external.connect(("1.1.1.1", 443))
except OSError as error:
    print("EXTERNAL_EGRESS_BLOCKED", error.errno)
else:
    raise SystemExit("external TCP connection unexpectedly succeeded")
finally:
    external.close()
PY
INNER

sudo -n bwrap \
  --unshare-net \
  --cap-add CAP_NET_ADMIN \
  --ro-bind / / \
  --bind "$WORKDIR" "$WORKDIR" \
  --dev-bind /dev /dev \
  --proc /proc \
  /bin/bash "$WORKDIR/inside.sh" "$WORKDIR" "$INFERD_BIN" "$MODEL" "$CHAT_MODEL" "$LLAMA_SERVER"
