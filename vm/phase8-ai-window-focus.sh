#!/usr/bin/env bash
# Ensure ordinary window focus changes on one output do not dismiss the AI drawer.
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PANEL_BIN="${EVERMORE_PANEL_BIN:-$REPO_ROOT/target/debug/evermore-panel}"
USER_ID="$(id -u)"
RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$USER_ID}"
WAYLAND_DISPLAY_NAME="${WAYLAND_DISPLAY:-wayland-1}"
SWAY_PID="$(pgrep -xo sway || true)"
SWAY_SOCKET="${SWAYSOCK:-}"
if [[ -z "$SWAY_SOCKET" && -n "$SWAY_PID" ]]; then
  SWAY_SOCKET="$RUNTIME_DIR/sway-ipc.$USER_ID.$SWAY_PID.sock"
fi

[[ -x "$PANEL_BIN" ]] || { echo "missing panel binary: $PANEL_BIN" >&2; exit 1; }
command -v foot >/dev/null || { echo "foot is required for the focus check" >&2; exit 1; }
[[ -S "$SWAY_SOCKET" ]] || { echo "no active Sway IPC socket" >&2; exit 1; }

WORKDIR="$(mktemp -d /tmp/evermore-p8-window-focus.XXXXXX)"
FOOT_PID=""
PANEL_PID=""
cleanup() {
  [[ -n "$PANEL_PID" ]] && kill "$PANEL_PID" 2>/dev/null || true
  [[ -n "$FOOT_PID" ]] && kill "$FOOT_PID" 2>/dev/null || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

export XDG_RUNTIME_DIR WAYLAND_DISPLAY="$WAYLAND_DISPLAY_NAME" SWAYSOCK="$SWAY_SOCKET"
foot --title="Phase 8 focus fixture" sleep 60 >"$WORKDIR/foot.log" 2>&1 &
FOOT_PID=$!
for _ in $(seq 1 40); do
  TREE="$(swaymsg -s "$SWAY_SOCKET" -t get_tree -r)"
  if python3 - "$TREE" <<'PY'
import json, sys
def walk(node):
    yield node
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        yield from walk(child)
raise SystemExit(not any(node.get("name") == "Phase 8 focus fixture" for node in walk(json.loads(sys.argv[1]))))
PY
  then break; fi
  sleep 0.1
done

"$PANEL_BIN" --ai >"$WORKDIR/panel.log" 2>&1 &
PANEL_PID=$!
sleep 2
kill -0 "$PANEL_PID" || { cat "$WORKDIR/panel.log" >&2; exit 1; }
swaymsg -s "$SWAY_SOCKET" '[title="Phase 8 focus fixture"] focus' >/dev/null
sleep 0.4
kill -0 "$PANEL_PID" || {
  cat "$WORKDIR/panel.log" >&2
  echo "AI drawer closed when focus moved to another window on the same output" >&2
  exit 1
}
TREE="$(swaymsg -s "$SWAY_SOCKET" -t get_tree -r)"
python3 - "$TREE" <<'PY'
import json, sys
tree = json.loads(sys.argv[1])
def walk(node):
    yield node
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        yield from walk(child)
assert any(node.get("name") == "Phase 8 focus fixture" and node.get("focused") for node in walk(tree)), "focus did not move to fixture window"
assert any(node.get("name") == "Evermore AI" for node in walk(tree)), "AI drawer disappeared"
PY
echo "AI_DRAWER_REMAINS_OPEN_ON_SAME_OUTPUT_FOCUS_OK"
