#!/usr/bin/env bash
# Exercise a real local model proposal in the AI drawer, then submit it through
# the inline card and approve it in the signed owner review UI. All user state,
# relay data, and settings live in a disposable XDG tree.
set -euo pipefail

# The kernel session keyring is inherited by descendants, but an SSH login
# may not have created one for its shell. In that case each short-lived
# keygen command gets an isolated auto-created keyring. Give this entire test
# one explicit session keyring so keygen, signer, and the UI's signer socket
# all see the same disposable identities.
if [[ "${EVERMORE_PHASE8_KEYRING_SESSION:-}" != "1" ]]; then
    command -v keyctl >/dev/null || { echo "keyctl is required for the Phase 8 signer test" >&2; exit 1; }
    exec keyctl session evermore-phase8-test env EVERMORE_PHASE8_KEYRING_SESSION=1 bash "$0" "$@"
fi

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${EVERMORE_PHASE8_BIN_DIR:-$REPO_ROOT/target/debug}"
RELAY_BIN="${EVERMORE_PHASE8_RELAY_BIN:-/tmp/evermore-relay-bin}"
MODEL_ROOT="${EVERMORE_PHASE8_MODEL_ROOT:-$HOME/.cache/evermore-p8-qwen/models}"
MODEL_NAME="${EVERMORE_PHASE8_MODEL_NAME:-evermore/p8.test.qwen2.5-1.5b:1.0.0}"
RELAY_PORT="${EVERMORE_PHASE8_RELAY_PORT:-$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')}"
TEST_HOME="$(mktemp -d /tmp/evermore-phase8-ai-proposal.XXXXXX)"
XDG_RUNTIME_DIR="$TEST_HOME/runtime"
XDG_CONFIG_HOME="$TEST_HOME/config"
XDG_DATA_HOME="$TEST_HOME/data"
XDG_STATE_HOME="$TEST_HOME/state"
XDG_CACHE_HOME="$TEST_HOME/cache"
RELAY_URL="ws://127.0.0.1:$RELAY_PORT"
RUN_ID="$(date +%s)-$$"
OWNER_ID="00-p8-ai-owner-$RUN_ID"
AGENT_ID="10-p8-ai-agent-$RUN_ID"
SERVICE_ID="20-p8-ai-service-$RUN_ID"
RELAY_PID=""
SIGNER_PID=""
SETTINGS_PID=""
OPERATIONS_PID=""
INFER_PID=""
SWAY_PID=""
PANEL_PID=""
CONTEXT_PID=""
REVIEW_PID=""

stop_group() {
    local pid="$1"
    [[ -n "$pid" ]] || return 0
    if kill -0 "$pid" 2>/dev/null; then
        kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
}
cleanup() {
    stop_group "$REVIEW_PID"
    stop_group "$PANEL_PID"
    stop_group "$SWAY_PID"
    stop_group "$INFER_PID"
    stop_group "$CONTEXT_PID"
    stop_group "$OPERATIONS_PID"
    stop_group "$SETTINGS_PID"
    stop_group "$SIGNER_PID"
    stop_group "$RELAY_PID"
    if [[ "${EVERMORE_PHASE8_KEEP_TEST_HOME:-}" == "1" ]]; then
        echo "PRESERVED_TEST_HOME=$TEST_HOME"
    else
        rm -rf -- "$TEST_HOME"
    fi
}
trap cleanup EXIT INT TERM

for binary in evermore-signer evermore-settingsd evermore-operationsd evermore-inferd evermore-contextd evermore-panel; do
    [[ -x "$BIN_DIR/$binary" ]] || { echo "missing binary: $BIN_DIR/$binary" >&2; exit 1; }
done
[[ -x "$RELAY_BIN" ]] || { echo "missing local relay binary: $RELAY_BIN" >&2; exit 1; }
[[ -f "$MODEL_ROOT/qwen2.5-test-1.0.0/manifest.json" && -f "$MODEL_ROOT/qwen2.5-test-1.0.0/base.gguf" ]] || {
    echo "missing temporary Phase 8 Qwen model under $MODEL_ROOT" >&2; exit 1;
}
for command in sway swaymsg wtype grim python3 curl swaync; do
    command -v "$command" >/dev/null || { echo "missing command: $command" >&2; exit 1; }
done

export PATH="$BIN_DIR:$PATH"
export XDG_RUNTIME_DIR XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/$(id -u)/bus}"
export EVERMORE_RELAY_URL="$RELAY_URL"
unset EVERMORE_CONTEXT_SOURCES EVERMORE_INDEX_FILES EVERMORE_SETTINGS_FILE
mkdir -m 0700 -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

mkdir -m 0700 -p "$XDG_CONFIG_HOME/evermore"
cat >"$XDG_CONFIG_HOME/evermore/panel-ai.json" <<PREFS
{"model":"$MODEL_NAME","backend":"cpu"}
PREFS
chmod 0600 "$XDG_CONFIG_HOME/evermore/panel-ai.json"

HOME="$TEST_HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" XDG_DATA_HOME="$XDG_DATA_HOME" \
    "$REPO_ROOT/packaging/sway/install-desktop.sh" >/dev/null
printf 'set $mod Mod4\ninclude %s\ninclude %s\nfor_window [title="Review operation request"] floating enable\nfor_window [title="Review operation request"] resize set width 620 px height 460 px\nfor_window [title="Review operation request"] move position center\n' \
    "$XDG_CONFIG_HOME/sway/evermore-desktop.conf" \
    "$XDG_CONFIG_HOME/sway/evermore-waybar.conf" \
    > "$XDG_CONFIG_HOME/sway/config"

EVERMORE_RELAY_PORT="$RELAY_PORT" EVERMORE_RELAY_DATA_DIR="$TEST_HOME/relay-data" \
    setsid "$RELAY_BIN" >"$TEST_HOME/relay.log" 2>&1 &
RELAY_PID=$!
for _ in $(seq 1 100); do
    if curl --silent --fail "http://127.0.0.1:$RELAY_PORT/" >/dev/null 2>&1; then break; fi
    kill -0 "$RELAY_PID" 2>/dev/null || { cat "$TEST_HOME/relay.log" >&2; exit 1; }
    sleep 0.1
done
kill -0 "$RELAY_PID" 2>/dev/null || { cat "$TEST_HOME/relay.log" >&2; exit 1; }

OWNER_KEYGEN="$("$BIN_DIR/evermore-signer" keygen "$OWNER_ID")"
AGENT_KEYGEN="$("$BIN_DIR/evermore-signer" keygen "$AGENT_ID")"
"$BIN_DIR/evermore-signer" keygen "$SERVICE_ID" >/dev/null
OWNER_PUBKEY="$(awk '$1 == "pubkey:" { print $2 }' <<<"$OWNER_KEYGEN")"
AGENT_PUBKEY="$(awk '$1 == "pubkey:" { print $2 }' <<<"$AGENT_KEYGEN")"
[[ -n "$OWNER_PUBKEY" && -n "$AGENT_PUBKEY" ]] || { echo "test identities were not created" >&2; exit 1; }

"$BIN_DIR/evermore-signer" serve --socket "$XDG_RUNTIME_DIR/evermore/signer.sock" \
    >"$TEST_HOME/signer.log" 2>&1 &
SIGNER_PID=$!
for _ in $(seq 1 50); do
    [[ -S "$XDG_RUNTIME_DIR/evermore/signer.sock" ]] && break
    kill -0 "$SIGNER_PID" 2>/dev/null || { cat "$TEST_HOME/signer.log" >&2; exit 1; }
    sleep 0.1
done
[[ -S "$XDG_RUNTIME_DIR/evermore/signer.sock" ]] || { echo "test signer did not start" >&2; exit 1; }
python3 - "$XDG_RUNTIME_DIR/evermore/signer.sock" "$OWNER_ID" "$AGENT_ID" <<'PY'
import json, socket, sys
sock=socket.socket(socket.AF_UNIX); sock.settimeout(3); sock.connect(sys.argv[1])
sock.sendall(b'{"method":"list_identities"}\n')
response=json.loads(sock.makefile().readline())
names=response.get("data", [])
assert response.get("status")=="ok" and sys.argv[2] in names and sys.argv[3] in names, response
print("SIGNER_DAEMON_HAS_TEST_IDENTITIES_OK", names)
PY

setsid "$BIN_DIR/evermore-settingsd" --identity "$OWNER_ID" --relay "$RELAY_URL" \
    >"$TEST_HOME/settingsd.log" 2>&1 &
SETTINGS_PID=$!
setsid "$BIN_DIR/evermore-operationsd" serve --owner-identity "$OWNER_ID" --service-identity "$SERVICE_ID" \
    >"$TEST_HOME/operationsd.log" 2>&1 &
OPERATIONS_PID=$!
for _ in $(seq 1 80); do
    [[ -S "$XDG_RUNTIME_DIR/evermore/settingsd.sock" && -S "$XDG_RUNTIME_DIR/evermore/operationsd.sock" ]] && break
    if ! kill -0 "$SETTINGS_PID" 2>/dev/null || ! kill -0 "$OPERATIONS_PID" 2>/dev/null; then
        cat "$TEST_HOME/settingsd.log" "$TEST_HOME/operationsd.log" >&2
        exit 1
    fi
    sleep 0.1
done
[[ -S "$XDG_RUNTIME_DIR/evermore/settingsd.sock" && -S "$XDG_RUNTIME_DIR/evermore/operationsd.sock" ]] || {
    echo "settings or operations daemon did not create its socket" >&2
    exit 1
}

"$BIN_DIR/evermore-operationsd" grant "$AGENT_PUBKEY" settings.set --identity "$OWNER_ID" \
    >"$TEST_HOME/grant.json"
python3 - "$TEST_HOME/grant.json" <<'PY'
import json, sys
assert json.load(open(sys.argv[1], encoding="utf-8")).get("grant_published") is True
PY
echo "OWNER_GRANTED_SETTINGS_SET_TO_AI_ACTOR_OK"

EVERMORE_MODEL_DIRS="$MODEL_ROOT" \
EVERMORE_MODEL_IMPORT_DIR="$MODEL_ROOT" \
EVERMORE_LLAMA_SERVER_BIN=/usr/bin/llama-server \
EVERMORE_INFER_MAX_MODELS=1 \
EVERMORE_INFER_READY_TIMEOUT_SECS=240 \
    setsid "$BIN_DIR/evermore-inferd" >"$TEST_HOME/inferd.log" 2>&1 &
INFER_PID=$!
for _ in $(seq 1 80); do
    [[ -S "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" ]] && break
    kill -0 "$INFER_PID" 2>/dev/null || { cat "$TEST_HOME/inferd.log" >&2; exit 1; }
    sleep 0.1
done
[[ -S "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" ]] || { echo "inference daemon did not start" >&2; exit 1; }
python3 - "$XDG_RUNTIME_DIR/evermore/ai-inferd.sock" "$MODEL_NAME" <<'PY'
import json, socket, sys
sock = socket.socket(socket.AF_UNIX)
sock.connect(sys.argv[1])
sock.sendall(b'{"method":"list_models"}\n')
response = json.loads(sock.makefile().readline())
models = response.get("models", [])
assert any(item.get("name") == sys.argv[2] for item in models), models
print("TEMP_QWEN_MODEL_DISCOVERED_OK")
PY
EVERMORE_CONTEXT_SOURCES='[]' setsid "$BIN_DIR/evermore-contextd" \
    >"$TEST_HOME/contextd.log" 2>&1 &
CONTEXT_PID=$!
for _ in $(seq 1 50); do
    [[ -S "$XDG_RUNTIME_DIR/evermore/ai-contextd.sock" ]] && break
    kill -0 "$CONTEXT_PID" 2>/dev/null || { cat "$TEST_HOME/contextd.log" >&2; exit 1; }
    sleep 0.1
done
[[ -S "$XDG_RUNTIME_DIR/evermore/ai-contextd.sock" ]] || { echo "context daemon did not start" >&2; exit 1; }

setsid env \
    HOME="$TEST_HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" XDG_DATA_HOME="$XDG_DATA_HOME" \
    XDG_STATE_HOME="$XDG_STATE_HOME" XDG_CACHE_HOME="$XDG_CACHE_HOME" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
    DBUS_SESSION_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" \
    WLR_BACKENDS=headless,libinput WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman \
    sway -c "$XDG_CONFIG_HOME/sway/config" -d >"$TEST_HOME/sway.log" 2>&1 </dev/null &
SWAY_PID=$!
SWAY_SOCKET="$XDG_RUNTIME_DIR/sway-ipc.$(id -u).$SWAY_PID.sock"
for _ in $(seq 1 80); do
    if swaymsg -s "$SWAY_SOCKET" -t get_seats -r >"$TEST_HOME/seats.json" 2>/dev/null; then break; fi
    kill -0 "$SWAY_PID" 2>/dev/null || { cat "$TEST_HOME/sway.log" >&2; exit 1; }
    sleep 0.1
done
grep -q 'AT_Translated_Set_2_keyboard' "$TEST_HOME/seats.json" || {
    cat "$TEST_HOME/seats.json" >&2
    echo "temporary Sway session did not attach the VM keyboard" >&2
    exit 1
}
WAYLAND_DISPLAY="$(sed -n "s/.*Running compositor on wayland display '\([^']*\)'.*/\1/p" "$TEST_HOME/sway.log" | tail -1)"
[[ -n "$WAYLAND_DISPLAY" ]] || { echo "could not determine temporary Wayland display" >&2; exit 1; }
export WAYLAND_DISPLAY SWAYSOCK="$SWAY_SOCKET"

# Open the real drawer through its configured keyboard binding.
wtype -M logo -k i -m logo
for _ in $(seq 1 80); do
    swaymsg -s "$SWAY_SOCKET" -t get_tree -r >"$TEST_HOME/tree.json" 2>/dev/null || true
    if python3 - "$TEST_HOME/tree.json" <<'PY'
import json, sys
try: root=json.load(open(sys.argv[1]))
except Exception: raise SystemExit(1)
def walk(node):
 yield node
 for child in node.get("nodes", [])+node.get("floating_nodes", []): yield from walk(child)
raise SystemExit(0 if any(n.get("name")=="Evermore AI" for n in walk(root)) else 1)
PY
    then break; fi
    sleep 0.1
done
if ! python3 - "$TEST_HOME/tree.json" <<'PY'
import json, sys
def walk(node):
 yield node
 for child in node.get("nodes", [])+node.get("floating_nodes", []): yield from walk(child)
root=json.load(open(sys.argv[1]))
raise SystemExit(0 if any(n.get("name")=="Evermore AI" for n in walk(root)) else 1)
PY
then
    cat "$TEST_HOME/sway.log" >&2
    echo "Super+I did not open the AI drawer" >&2
    exit 1
fi
for candidate in $(pgrep -u "$(id -u)" -x evermore-panel || true); do
    if [[ -r "/proc/$candidate/environ" ]] \
        && tr '\0' '\n' <"/proc/$candidate/environ" \
            | grep -Fxq "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR"; then
        PANEL_PID="$candidate"
        break
    fi
done
[[ -n "$PANEL_PID" ]] || { echo "could not identify the temporary AI drawer process" >&2; exit 1; }
sleep 1
if [[ -n "${EVERMORE_PHASE8_AI_DRAWER_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE8_AI_DRAWER_SCREENSHOT"
fi
if [[ "${EVERMORE_PHASE8_STOP_AFTER_AI_OPEN:-}" == "1" ]]; then
    echo "AI_DRAWER_OPENED_AND_MODEL_DISCOVERED_OK"
    exit 0
fi

# The panel is placed at the focused output's upper-left corner. Click the
# prompt editor, enter a deterministic write request, then click Send locally.
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set 170 "${EVERMORE_PHASE8_PROMPT_Y:-300}" >/dev/null
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
sleep 0.1
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null
wtype "Set desktop theme setting domain 'theme' key 'mode' to dark."
sleep 0.25
if [[ -n "${EVERMORE_PHASE8_BEFORE_SEND_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE8_BEFORE_SEND_SCREENSHOT"
fi
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set "${EVERMORE_PHASE8_SEND_X:-65}" "${EVERMORE_PHASE8_SEND_Y:-358}" >/dev/null
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
sleep 0.1
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null

SESSION_FILE="$XDG_DATA_HOME/evermore/ai-sessions.json"
PROPOSAL_JSON="$TEST_HOME/proposal.json"
FOUND=0
RESPONSE_TIMEOUT_SECONDS="${EVERMORE_PHASE8_RESPONSE_TIMEOUT_SECONDS:-300}"
for _ in $(seq 1 "$((RESPONSE_TIMEOUT_SECONDS * 2))"); do
    if python3 - "$SESSION_FILE" "$PROPOSAL_JSON" <<'PY'
import json, os, sys
path, out = sys.argv[1:]
try: sessions=json.load(open(path, encoding="utf-8"))
except (OSError, json.JSONDecodeError): raise SystemExit(1)
for session in sessions:
 for message in session.get("messages", []):
  content=message.get("content", "")
  for line in content.splitlines():
   if line.strip().startswith("EVERMORE_PROPOSAL:"):
    try: proposal=json.loads(line.split(":", 1)[1].strip())
    except json.JSONDecodeError: continue
    if proposal.get("operation")=="settings.set":
     json.dump({"proposal":proposal,"session":session.get("id")}, open(out,"w",encoding="utf-8"))
     raise SystemExit(0)
raise SystemExit(1)
PY
    then FOUND=1; break; fi
    if ! kill -0 "$INFER_PID" 2>/dev/null; then cat "$TEST_HOME/inferd.log" >&2; exit 1; fi
    sleep 0.5
done
[[ "$FOUND" == 1 ]] || {
    grim "$TEST_HOME/model-response-timeout.png" || true
    cat "$TEST_HOME/inferd.log" >&2
    cat "$TEST_HOME/sway.log" >&2
    echo "real model did not return a settings.set proposal marker within ${RESPONSE_TIMEOUT_SECONDS}s" >&2
    exit 1
}
python3 - "$PROPOSAL_JSON" <<'PY'
import json, sys
obj=json.load(open(sys.argv[1], encoding="utf-8"))
p=obj["proposal"]
assert p=={"operation":"settings.set","arguments":{"domain":"theme","key":"mode","value":"dark"}}, p
print("REAL_MODEL_PROPOSAL_SCHEMA_AND_ARGUMENTS_OK", json.dumps(p, sort_keys=True))
PY
for _ in $(seq 1 "${EVERMORE_PHASE8_CARD_SCROLL_STEPS:-0}"); do
    swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set 240 420 >/dev/null
    swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button5 >/dev/null
    sleep 0.05
    swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button5 >/dev/null
done
sleep 0.5
if [[ -n "${EVERMORE_PHASE8_AI_CARD_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE8_AI_CARD_SCREENSHOT"
fi
echo "REAL_MODEL_PROPOSAL_CARD_RENDERED_OK"
if [[ "${EVERMORE_PHASE8_STOP_AFTER_CARD:-}" == "1" ]]; then
    echo "TEST_STOPPED_AFTER_REAL_MODEL_CARD=1"
    exit 0
fi

# The panel now selects the first loaded identity with an active, published
# grant for this operation. The assertion below verifies the submitted signer
# is exactly the test agent; no unfiltered identity-picker navigation is used.
sleep 0.2
SUBMIT_X="${EVERMORE_PHASE8_SUBMIT_X:-130}"
SUBMIT_Y="${EVERMORE_PHASE8_SUBMIT_Y:-506}"
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set "$SUBMIT_X" "$SUBMIT_Y" >/dev/null
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
sleep 0.1
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null

PENDING_JSON="$TEST_HOME/pending.json"
OPERATION_ID=""
for _ in $(seq 1 100); do
    python3 - "$XDG_RUNTIME_DIR/evermore/operationsd.sock" "$PENDING_JSON" <<'PY' 2>/dev/null || true
import json, os, socket, sys
sock=socket.socket(socket.AF_UNIX); sock.settimeout(2); sock.connect(sys.argv[1])
sock.sendall(b'{"method":"list_pending_operations"}\n')
response=json.loads(sock.makefile().readline())
if response.get("status")!="ok": raise SystemExit(1)
pending=response.get("data", [])
json.dump(pending, open(sys.argv[2],"w",encoding="utf-8"))
PY
    OPERATION_ID="$(python3 - "$PENDING_JSON" <<'PY'
import json,sys
try: rows=json.load(open(sys.argv[1],encoding="utf-8"))
except Exception: rows=[]
for item in rows:
 if item.get("operation")=="settings.set" and item.get("arguments")=={"domain":"theme","key":"mode","value":"dark"}:
  print(item.get("operation_id","")); break
PY
)"
    [[ -n "$OPERATION_ID" ]] && break
    sleep 0.2
done
[[ -n "$OPERATION_ID" ]] || {
    grim "$TEST_HOME/proposal-submit-failure.png" || true
    echo "inline proposal card did not submit the signed settings.set request" >&2
    exit 1
}
echo "INLINE_CARD_SUBMITTED_SIGNED_REQUEST_OK operation_id=$OPERATION_ID"

# Confirm the operation is still pending before opening the owner review UI.
STATUS_JSON="$("$BIN_DIR/evermore-operationsd" status "$OPERATION_ID")"
python3 - "$STATUS_JSON" "$AGENT_PUBKEY" "$OWNER_PUBKEY" <<'PY'
import json,sys
response=json.loads(sys.argv[1])
status=response.get("data", response)
assert status["status"]=="pending", status
assert status["operation"]=="settings.set", status
assert status["actor_pubkey"]==sys.argv[2], status
request_owner=next((tag[1] for tag in status["request_event"].get("tags", []) if len(tag)>1 and tag[0]=="p"), None)
assert request_owner==sys.argv[3], status
PY

setsid "$BIN_DIR/evermore-panel" --approval "$OPERATION_ID" \
    >"$TEST_HOME/review.log" 2>&1 </dev/null &
REVIEW_PID=$!
for _ in $(seq 1 80); do
    swaymsg -s "$SWAY_SOCKET" -t get_tree -r >"$TEST_HOME/review-tree.json" 2>/dev/null || true
    if python3 - "$TEST_HOME/review-tree.json" <<'PY'
import json,sys
def walk(n):
 yield n
 for c in n.get("nodes",[])+n.get("floating_nodes",[]): yield from walk(c)
try: root=json.load(open(sys.argv[1]))
except Exception: raise SystemExit(1)
raise SystemExit(0 if any(n.get("name")=="Review operation request" for n in walk(root)) else 1)
PY
    then break; fi
    kill -0 "$REVIEW_PID" 2>/dev/null || { cat "$TEST_HOME/review.log" >&2; exit 1; }
    sleep 0.1
done
sleep 0.5
if [[ -n "${EVERMORE_PHASE8_AI_REVIEW_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE8_AI_REVIEW_SCREENSHOT"
fi
# Same 1280x720 centered review layout validated by the Phase 8 deep-link test.
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set 385 472 >/dev/null
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
sleep 0.1
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null

STATUS=""
for _ in $(seq 1 120); do
    STATUS_JSON="$("$BIN_DIR/evermore-operationsd" status "$OPERATION_ID" 2>/dev/null || true)"
    STATUS="$(python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("data",d).get("status",""))' <<<"$STATUS_JSON" 2>/dev/null || true)"
    [[ "$STATUS" == "succeeded" ]] && break
    sleep 0.25
done
[[ "$STATUS" == "succeeded" ]] || {
    grim "$TEST_HOME/owner-approval-failure.png" || true
    cat "$TEST_HOME/operationsd.log" "$TEST_HOME/settingsd.log" "$TEST_HOME/review.log" >&2
    echo "owner approval did not execute the model proposal (status=$STATUS)" >&2
    exit 1
}
python3 - "$STATUS_JSON" <<'PY'
import json,sys
response=json.loads(sys.argv[1])
record=response.get("data", response)
assert record["status"]=="succeeded", record
assert record.get("result",{}).get("domain")=="theme", record
PY
SETTINGS_JSON="$(python3 - "$XDG_RUNTIME_DIR/evermore/settingsd.sock" <<'PY'
import json,socket,sys
s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])
s.sendall(b'{"method":"get","params":{"domain":"theme","key":"mode"}}\n')
print(s.makefile().readline(),end="")
PY
)"
python3 - "$SETTINGS_JSON" <<'PY'
import json,sys
data=json.loads(sys.argv[1])
assert data.get("status")=="ok", data
assert data.get("data")=="dark", data
print("OWNER_APPROVED_MODEL_PROPOSAL_AND_THEME_CHANGED_OK")
PY
