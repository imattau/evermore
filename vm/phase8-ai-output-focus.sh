#!/usr/bin/env bash
# Confirm the AI drawer closes when Sway focus moves to another output.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PANEL_BIN="${EVERMORE_PANEL_BIN:-$REPO_ROOT/target/debug/evermore-panel}"
USER_ID="$(id -u)"
RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$USER_ID}"
WAYLAND_DISPLAY_NAME="${WAYLAND_DISPLAY:-wayland-1}"
WAYLAND_SOCKET="$WAYLAND_DISPLAY_NAME"
if [[ "$WAYLAND_SOCKET" != /* ]]; then
  WAYLAND_SOCKET="$RUNTIME_DIR/$WAYLAND_SOCKET"
fi
SWAY_PID="$(pgrep -xo sway || true)"
SWAY_SOCKET="${SWAYSOCK:-}"
if [[ -z "$SWAY_SOCKET" && -n "$SWAY_PID" ]]; then
  SWAY_SOCKET="$RUNTIME_DIR/sway-ipc.$USER_ID.$SWAY_PID.sock"
fi

[[ -x "$PANEL_BIN" ]] || { echo "missing panel binary: $PANEL_BIN" >&2; exit 1; }
[[ -S "$WAYLAND_SOCKET" ]] || {
  echo "no active Wayland display at $WAYLAND_SOCKET" >&2
  exit 1
}
[[ -S "$SWAY_SOCKET" ]] || { echo "no active Sway IPC socket" >&2; exit 1; }

BEFORE="$(swaymsg -s "$SWAY_SOCKET" -t get_outputs -r)"
FOCUSED_OUTPUT="$(python3 -c 'import json,sys; print(next(o["name"] for o in json.loads(sys.argv[1]) if o.get("focused")))' "$BEFORE")"
NEW_OUTPUT=""
PANEL_PID=""
WORKDIR="$(mktemp -d /tmp/evermore-p8-output-focus.XXXXXX)"
cleanup() {
  [[ -n "$PANEL_PID" ]] && kill "$PANEL_PID" 2>/dev/null || true
  if [[ -n "$NEW_OUTPUT" ]]; then
    swaymsg -s "$SWAY_SOCKET" output "$NEW_OUTPUT" unplug >/dev/null 2>&1 || true
  fi
  swaymsg -s "$SWAY_SOCKET" focus output "$FOCUSED_OUTPUT" >/dev/null 2>&1 || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

swaymsg -s "$SWAY_SOCKET" create_output >/dev/null
for _ in $(seq 1 20); do
  AFTER="$(swaymsg -s "$SWAY_SOCKET" -t get_outputs -r)"
  NEW_OUTPUT="$(python3 -c 'import json,sys; before={o["name"] for o in json.loads(sys.argv[1])}; after=json.loads(sys.argv[2]); print(next((o["name"] for o in after if o["name"] not in before), ""))' "$BEFORE" "$AFTER")"
  [[ -n "$NEW_OUTPUT" ]] && break
  sleep 0.1
done
[[ -n "$NEW_OUTPUT" ]] || { echo "Sway did not create a test output" >&2; exit 1; }
swaymsg -s "$SWAY_SOCKET" focus output "$NEW_OUTPUT" >/dev/null

export XDG_RUNTIME_DIR="$RUNTIME_DIR"
export WAYLAND_DISPLAY="$WAYLAND_DISPLAY_NAME"
export SWAYSOCK="$SWAY_SOCKET"
"$PANEL_BIN" --ai >"$WORKDIR/panel.log" 2>&1 &
PANEL_PID=$!
sleep 2
kill -0 "$PANEL_PID" || { cat "$WORKDIR/panel.log" >&2; exit 1; }
TREE="$(swaymsg -s "$SWAY_SOCKET" -t get_tree -r)"
python3 - "$TREE" "$NEW_OUTPUT" <<'PY'
import json, sys
tree, expected = json.loads(sys.argv[1]), sys.argv[2]
def contains_title(node):
    return node.get("name") == "Evermore AI" or any(
        contains_title(child) for child in node.get("nodes", []) + node.get("floating_nodes", [])
    )
outputs = [node for node in tree.get("nodes", []) if node.get("type") == "output"]
actual = next((node.get("name") for node in outputs if contains_title(node)), None)
assert actual == expected, f"drawer opened on {actual!r}; focused output was {expected!r}"
PY
echo "AI_DRAWER_OPENS_ON_FOCUSED_OUTPUT_OK" "$NEW_OUTPUT"
swaymsg -s "$SWAY_SOCKET" focus output "$FOCUSED_OUTPUT" >/dev/null

for _ in $(seq 1 40); do
  STATE="$(ps -o stat= -p "$PANEL_PID" 2>/dev/null || true)"
  if [[ -z "$STATE" || "$STATE" == Z* ]]; then
    wait "$PANEL_PID" 2>/dev/null || true
    PANEL_PID=""
    echo "AI_DRAWER_CLOSED_ON_OUTPUT_CHANGE_OK" "$FOCUSED_OUTPUT" "->" "$NEW_OUTPUT"
    exit 0
  fi
  sleep 0.25
done
cat "$WORKDIR/panel.log" >&2
echo "AI drawer remained open after focus moved to $NEW_OUTPUT" >&2
exit 1
