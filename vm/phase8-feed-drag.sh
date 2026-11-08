#!/usr/bin/env bash
# Exercise the feed drawer's note handoff in a disposable headless Sway
# session: a mock relay supplies one note, dragging it copies the text to the
# clipboard, and "Attach to chat" hands it to the AI drawer as one-turn
# context. The drop-over-AI half of the drag needs real pointer motion, which
# Sway's `seat cursor` commands do not synthesize during a button grab; the
# window-rectangle check itself is covered by unit tests.
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PANEL_BIN="${EVERMORE_PANEL_BIN:-$REPO_ROOT/target/debug/evermore-panel}"
USER_ID="$(id -u)"
TEST_HOME="$(mktemp -d /tmp/evermore-p8-feed-drag.XXXXXX)"
XDG_RUNTIME_DIR="$TEST_HOME/runtime"
XDG_CONFIG_HOME="$TEST_HOME/config"
XDG_DATA_HOME="$TEST_HOME/data"
XDG_STATE_HOME="$TEST_HOME/state"
XDG_CACHE_HOME="$TEST_HOME/cache"
RELAY_PORT="${EVERMORE_PHASE8_RELAY_PORT:-$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')}"
NOTE_TEXT="Hello from the mock relay. Drag me onto the AI drawer."
SWAY_PID=""
RELAY_PID=""
AI_PID=""
FEED_PID=""

stop_group() {
    local pid="$1"
    [[ -n "$pid" ]] || return 0
    if kill -0 "$pid" 2>/dev/null; then
        kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
}
cleanup() {
    stop_group "$FEED_PID"
    stop_group "$AI_PID"
    stop_group "$RELAY_PID"
    stop_group "$SWAY_PID"
    rm -rf "$TEST_HOME"
}
trap cleanup EXIT INT TERM

[[ -x "$PANEL_BIN" ]] || { echo "missing panel binary: $PANEL_BIN" >&2; exit 1; }
for command in sway swaymsg python3 wl-paste; do
    command -v "$command" >/dev/null || { echo "missing command: $command" >&2; exit 1; }
done
python3 -c 'import websockets' 2>/dev/null || {
    echo "python websockets is required for the mock relay" >&2
    exit 1
}
if [[ -n "${EVERMORE_PHASE8_SCREENSHOT:-}" ]]; then
    command -v grim >/dev/null || { echo "missing command: grim" >&2; exit 1; }
fi

export HOME="$TEST_HOME"
export XDG_RUNTIME_DIR XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/$USER_ID/bus}"
mkdir -m 0700 -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" \
    "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

python3 - "$RELAY_PORT" "$NOTE_TEXT" <<'PY' &
import asyncio, json, sys
import websockets
from websockets.exceptions import ConnectionClosed

port = int(sys.argv[1])
content = sys.argv[2]
# A well-formed but unsigned event is enough: the feed client parses it as a
# note and never treats relay input as trusted for publishing.
event = {
    "id": "11" * 32,
    "pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    "created_at": 1_760_000_000,
    "kind": 1,
    "tags": [],
    "content": content,
    "sig": "00" * 64,
}

async def handler(websocket):
    try:
        async for message in websocket:
            try:
                data = json.loads(message)
            except json.JSONDecodeError:
                continue
            if data and data[0] == "REQ":
                await websocket.send(json.dumps(["EVENT", data[1], event]))
    except ConnectionClosed:
        pass

async def main():
    async with websockets.serve(handler, "127.0.0.1", port):
        await asyncio.Future()

asyncio.run(main())
PY
RELAY_PID=$!
for _ in $(seq 1 50); do
    if python3 - "$RELAY_PORT" <<'PY' 2>/dev/null
import socket, sys
s = socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=0.2)
s.close()
PY
    then break; fi
    kill -0 "$RELAY_PID" 2>/dev/null || { echo "mock relay did not start" >&2; exit 1; }
    sleep 0.1
done

"$REPO_ROOT/packaging/sway/install-desktop.sh" >/dev/null
printf 'set $mod Mod4\ninclude %s\n' \
    "$XDG_CONFIG_HOME/sway/evermore-desktop.conf" \
    > "$XDG_CONFIG_HOME/sway/config"

setsid env WLR_BACKENDS=headless,libinput WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman \
    sway -c "$XDG_CONFIG_HOME/sway/config" -d >"$TEST_HOME/sway.log" 2>&1 </dev/null &
SWAY_PID=$!
SWAY_SOCKET="$XDG_RUNTIME_DIR/sway-ipc.$USER_ID.$SWAY_PID.sock"
for _ in $(seq 1 80); do
    if swaymsg -s "$SWAY_SOCKET" -t get_outputs -r >"$TEST_HOME/outputs.json" 2>/dev/null; then break; fi
    kill -0 "$SWAY_PID" 2>/dev/null || { cat "$TEST_HOME/sway.log" >&2; exit 1; }
    sleep 0.1
done
WAYLAND_DISPLAY="$(sed -n "s/.*Running compositor on wayland display '\([^']*\)'.*/\1/p" "$TEST_HOME/sway.log" | tail -1)"
[[ -n "$WAYLAND_DISPLAY" ]] || { echo "could not determine temporary Wayland display" >&2; exit 1; }
export WAYLAND_DISPLAY SWAYSOCK="$SWAY_SOCKET"

start_feed() {
    EVERMORE_EXTERNAL_RELAYS="ws://127.0.0.1:$RELAY_PORT" \
        setsid "$PANEL_BIN" --nostr-feed >"$TEST_HOME/feed.log" 2>&1 &
    FEED_PID=$!
}

wait_for_drawers() {
    for _ in $(seq 1 80); do
        swaymsg -s "$SWAY_SOCKET" -t get_tree -r >"$TEST_HOME/tree.json" 2>/dev/null || true
        if python3 - "$TEST_HOME/tree.json" <<'PY'
import json, sys
try:
    tree = json.load(open(sys.argv[1]))
except Exception:
    raise SystemExit(1)
def walk(node):
    yield node
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        yield from walk(child)
names = {node.get("name") for node in walk(tree) if node.get("pid")}
raise SystemExit(0 if {"Evermore AI", "Evermore feed"} <= names else 1)
PY
        then return 0; fi
        kill -0 "$AI_PID" 2>/dev/null || { cat "$TEST_HOME/ai.log" >&2; exit 1; }
        kill -0 "$FEED_PID" 2>/dev/null || { cat "$TEST_HOME/feed.log" >&2; exit 1; }
        sleep 0.1
    done
    return 1
}

drawer_rects() {
    python3 - "$TEST_HOME/tree.json" >"$TEST_HOME/rects" <<'PY'
import json, sys
tree = json.load(open(sys.argv[1]))
def walk(node):
    yield node
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        yield from walk(child)
rects = {}
for node in walk(tree):
    if node.get("pid") and node.get("name") in ("Evermore feed", "Evermore AI"):
        r = node["rect"]
        rects[node["name"]] = (r["x"], r["y"])
feed = rects["Evermore feed"]
ai = rects["Evermore AI"]
print(feed[0], feed[1], ai[0], ai[1])
PY
    read -r FEED_X FEED_Y AI_X AI_Y <"$TEST_HOME/rects"
}

setsid "$PANEL_BIN" --ai >"$TEST_HOME/ai.log" 2>&1 &
AI_PID=$!
start_feed
wait_for_drawers

# Give the feed a moment to render the note card.
sleep 1

if [[ -n "${EVERMORE_PHASE8_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE8_SCREENSHOT"
fi

drawer_rects

# The drag start copies the note to the clipboard. Sway's synthetic cursor
# cannot move during the button grab, so this stops at the copy half; the
# release lands on the feed itself and attaches nothing.
swaymsg -s "$SWAY_SOCKET" '[title="Evermore feed"] focus' >/dev/null
swaymsg -s "$SWAY_SOCKET" "seat seat0 cursor set $((FEED_X + 90)) $((FEED_Y + 115))" >/dev/null
sleep 0.2
swaymsg -s "$SWAY_SOCKET" "seat seat0 cursor press button1" >/dev/null
sleep 1.0
swaymsg -s "$SWAY_SOCKET" "seat seat0 cursor release button1" >/dev/null
sleep 0.5

CLIPBOARD="$(wl-paste --no-newline 2>/dev/null || true)"
[[ "$CLIPBOARD" == "$NOTE_TEXT" ]] || {
    echo "clipboard does not contain the dragged note" >&2
    exit 1
}

# Restart the feed so the copy status line does not shift the Attach button,
# then use the explicit handoff path and confirm the AI drawer consumed it.
stop_group "$FEED_PID"
FEED_PID=""
start_feed
wait_for_drawers
sleep 1
drawer_rects
swaymsg -s "$SWAY_SOCKET" '[title="Evermore feed"] focus' >/dev/null
swaymsg -s "$SWAY_SOCKET" "seat seat0 cursor set $((FEED_X + 100)) $((FEED_Y + 174))" >/dev/null
sleep 0.2
swaymsg -s "$SWAY_SOCKET" "seat seat0 cursor press button1" >/dev/null
swaymsg -s "$SWAY_SOCKET" "seat seat0 cursor release button1" >/dev/null

SESSION_FILE="$XDG_DATA_HOME/evermore/ai-sessions.json"
for _ in $(seq 1 40); do
    if [[ -f "$SESSION_FILE" ]] && grep -q '"note:' "$SESSION_FILE"; then break; fi
    sleep 0.1
done
grep -q '"note:' "$SESSION_FILE" || {
    cat "$TEST_HOME/ai.log" >&2
    cat "$TEST_HOME/feed.log" >&2
    echo "attach did not reach the AI drawer" >&2
    exit 1
}

[[ ! -e "$XDG_RUNTIME_DIR/evermore/ai-note-handoff.json" ]] || {
    echo "handoff file was not consumed by the AI drawer" >&2
    exit 1
}

echo "FEED_DRAG_ATTACH_OK clipboard=note ai_context=note"
