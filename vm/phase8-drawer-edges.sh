#!/usr/bin/env bash
# Verify the two Evermore drawers are independent edge-anchored windows:
# the AI chat against the left edge and the Nostr feed against the right edge.
# Runs in a disposable headless Sway session inside the Arch VM.
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PANEL_BIN="${EVERMORE_PANEL_BIN:-$REPO_ROOT/target/debug/evermore-panel}"
USER_ID="$(id -u)"
TEST_HOME="$(mktemp -d /tmp/evermore-p8-drawer-edges.XXXXXX)"
XDG_RUNTIME_DIR="$TEST_HOME/runtime"
XDG_CONFIG_HOME="$TEST_HOME/config"
XDG_DATA_HOME="$TEST_HOME/data"
XDG_STATE_HOME="$TEST_HOME/state"
XDG_CACHE_HOME="$TEST_HOME/cache"
SWAY_PID=""
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
    stop_group "$SWAY_PID"
    rm -rf "$TEST_HOME"
}
trap cleanup EXIT INT TERM

[[ -x "$PANEL_BIN" ]] || { echo "missing panel binary: $PANEL_BIN" >&2; exit 1; }
for command in sway swaymsg python3; do
    command -v "$command" >/dev/null || { echo "missing command: $command" >&2; exit 1; }
done
if [[ -n "${EVERMORE_PHASE8_SCREENSHOT:-}" ]]; then
    command -v grim >/dev/null || { echo "missing command: grim" >&2; exit 1; }
fi

export HOME="$TEST_HOME"
export XDG_RUNTIME_DIR XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/$USER_ID/bus}"
mkdir -m 0700 -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" \
    "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

"$REPO_ROOT/packaging/sway/install-desktop.sh" >/dev/null
printf 'set $mod Mod4\ninclude %s\n' \
    "$XDG_CONFIG_HOME/sway/evermore-desktop.conf" \
    > "$XDG_CONFIG_HOME/sway/config"

setsid env WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman \
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

setsid "$PANEL_BIN" --ai >"$TEST_HOME/ai.log" 2>&1 &
AI_PID=$!
setsid "$PANEL_BIN" --nostr-feed >"$TEST_HOME/feed.log" 2>&1 &
FEED_PID=$!

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
    then break; fi
    kill -0 "$AI_PID" 2>/dev/null || { cat "$TEST_HOME/ai.log" >&2; exit 1; }
    kill -0 "$FEED_PID" 2>/dev/null || { cat "$TEST_HOME/feed.log" >&2; exit 1; }
    sleep 0.1
done

python3 - "$TEST_HOME/tree.json" <<'PY'
import json, sys

tree = json.load(open(sys.argv[1]))

def walk(node, workspace=None, output=None):
    if node.get("type") == "output":
        output = node
    if node.get("type") == "workspace":
        workspace = node
    yield node, workspace, output
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        yield from walk(child, workspace, output)

found = {}
for node, workspace, output in walk(tree):
    if node.get("pid") and node.get("name") in ("Evermore AI", "Evermore feed"):
        found.setdefault(node["name"], (node, workspace, output))

ai, ai_workspace, ai_output = found.get("Evermore AI", (None, None, None))
feed, feed_workspace, feed_output = found.get("Evermore feed", (None, None, None))
assert ai and ai_workspace and ai_output, "AI drawer window is missing"
assert feed and feed_workspace and feed_output, "Nostr feed window is missing"

def rect(node):
    r = node["rect"]
    return r["x"], r["y"], r["width"], r["height"]

ai_x, ai_y, ai_w, ai_h = rect(ai)
feed_x, feed_y, feed_w, feed_h = rect(feed)
_, ai_ws_y, _, _ = rect(ai_workspace)
output_x = rect(ai_output)[0]
feed_output_x, _, feed_output_w, _ = rect(feed_output)

assert ai_x == output_x, f"AI drawer is at x={ai_x}, expected output edge {output_x}"
assert ai_y == ai_ws_y, f"AI drawer is at y={ai_y}, expected workspace top {ai_ws_y}"
assert (ai_w, ai_h) == (400, 680), f"AI drawer is {ai_w}x{ai_h}, expected 400x680"

_, feed_ws_y, _, _ = rect(feed_workspace)
assert feed_x + feed_w == feed_output_x + feed_output_w, (
    f"feed right edge is {feed_x + feed_w}, expected output edge {feed_output_x + feed_output_w}"
)
assert feed_y == feed_ws_y, f"feed drawer is at y={feed_y}, expected workspace top {feed_ws_y}"
assert (feed_w, feed_h) == (340, 680), f"feed drawer is {feed_w}x{feed_h}, expected 340x680"
PY

if [[ -n "${EVERMORE_PHASE8_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE8_SCREENSHOT"
fi

echo "DRAWER_EDGES_OK ai=left/400px feed=right/340px"
