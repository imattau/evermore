#!/usr/bin/env bash
# Verify the file-manager desktop entry hands an explicitly selected text
# file to the AI drawer as an unsent one-turn context grant.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PANEL_BIN="${EVERMORE_PANEL_BIN:-$REPO_ROOT/target/debug/evermore-panel}"
INDEXD_BIN="${EVERMORE_INDEXD_BIN:-$REPO_ROOT/target/debug/evermore-indexd}"
USER_ID="$(id -u)"
RUNTIME_BASE="${XDG_RUNTIME_DIR:-/run/user/$USER_ID}"
WAYLAND_SOCKET="${WAYLAND_DISPLAY:-wayland-1}"
[[ "$WAYLAND_SOCKET" = /* ]] || WAYLAND_SOCKET="$RUNTIME_BASE/$WAYLAND_SOCKET"
SWAY_PID="$(pgrep -xo sway || true)"
SWAY_SOCKET="${SWAYSOCK:-}"
if [[ -z "$SWAY_SOCKET" && -n "$SWAY_PID" ]]; then
  SWAY_SOCKET="$RUNTIME_BASE/sway-ipc.$USER_ID.$SWAY_PID.sock"
fi

command -v desktop-file-validate >/dev/null || { echo "desktop-file-utils is required" >&2; exit 1; }
[[ -x "$PANEL_BIN" ]] || { echo "missing panel binary: $PANEL_BIN" >&2; exit 1; }
[[ -x "$INDEXD_BIN" ]] || { echo "missing index daemon: $INDEXD_BIN" >&2; exit 1; }
[[ -S "$WAYLAND_SOCKET" ]] || { echo "no Wayland socket at $WAYLAND_SOCKET" >&2; exit 1; }
[[ -S "$SWAY_SOCKET" ]] || { echo "no active Sway IPC socket" >&2; exit 1; }

desktop-file-validate \
  "$REPO_ROOT/packaging/applications/org.evermore.Panel.desktop" \
  "$REPO_ROOT/packaging/applications/org.evermore.SendToAI.desktop"

WORKDIR="$(mktemp -d /tmp/evermore-p8-send-to-ai.XXXXXX)"
INDEXD_PID=""
PANEL_PID=""
cleanup() {
  [[ -n "$PANEL_PID" ]] && kill "$PANEL_PID" 2>/dev/null || true
  [[ -n "$INDEXD_PID" ]] && kill "$INDEXD_PID" 2>/dev/null || true
  [[ -n "$PANEL_PID" ]] && wait "$PANEL_PID" 2>/dev/null || true
  [[ -n "$INDEXD_PID" ]] && wait "$INDEXD_PID" 2>/dev/null || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

export XDG_RUNTIME_DIR="$WORKDIR/runtime"
export XDG_CONFIG_HOME="$WORKDIR/config"
export XDG_DATA_HOME="$WORKDIR/data"
export XDG_STATE_HOME="$WORKDIR/state"
export EVERMORE_INDEX_STATE_DIR="$WORKDIR/index-state"
export WAYLAND_DISPLAY="$WAYLAND_SOCKET"
export SWAYSOCK="$SWAY_SOCKET"
unset EVERMORE_INDEX_FILES EVERMORE_CONTEXT_SOURCES
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME"
chmod 700 "$XDG_RUNTIME_DIR"

"$INDEXD_BIN" >"$WORKDIR/indexd.log" 2>&1 &
INDEXD_PID=$!
for _ in $(seq 1 50); do
  [[ -S "$XDG_RUNTIME_DIR/evermore/ai-indexd.sock" ]] && break
  sleep 0.1
done
[[ -S "$XDG_RUNTIME_DIR/evermore/ai-indexd.sock" ]] || {
  cat "$WORKDIR/indexd.log" >&2
  echo "index daemon failed to start" >&2
  exit 1
}

printf 'A selected local note for the Evermore AI drawer.\n' >"$WORKDIR/selected.md"
printf 'Private content that must stay outside AI context.\n' >"$WORKDIR/private.md"
: >"$WORKDIR/private.md.no-ai"

launch_drawer() {
  "$PANEL_BIN" --ai --file "$1" >"$WORKDIR/panel.log" 2>&1 &
  PANEL_PID=$!
  sleep 2
  kill -0 "$PANEL_PID" || { cat "$WORKDIR/panel.log" >&2; return 1; }
}

launch_drawer "$WORKDIR/selected.md"
if [[ -n "${EVERMORE_PHASE8_SEND_TO_AI_SCREENSHOT:-}" ]]; then
  grim "$EVERMORE_PHASE8_SEND_TO_AI_SCREENSHOT"
fi
if [[ -n "${EVERMORE_PHASE8_CONTEXT_TRAY_SCREENSHOT:-}" ]]; then
  swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set 80 510 >/dev/null
  swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
  sleep 0.1
  swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null
  sleep 0.25
  swaymsg -s "$SWAYSOCK" seat seat0 cursor set 100 605 >/dev/null
  swaymsg -s "$SWAYSOCK" seat seat0 cursor press button1 >/dev/null
  sleep 0.1
  swaymsg -s "$SWAYSOCK" seat seat0 cursor release button1 >/dev/null
  sleep 0.25
  grim "$EVERMORE_PHASE8_CONTEXT_TRAY_SCREENSHOT"
fi
python3 - "$XDG_DATA_HOME/evermore/ai-sessions.json" "$XDG_CONFIG_HOME/evermore/index-files.json" "$WORKDIR/selected.md" <<'PY'
import json
import sys
from pathlib import Path

sessions_path, scopes_path, selected = map(Path, sys.argv[1:])
sessions = json.loads(sessions_path.read_text())
selected = str(selected.resolve())
grant = next(
    (grant for session in sessions for grant in session["context"]
     if grant["source"] == "file:" + selected),
    None,
)
assert grant is not None, sessions
assert grant["scope"] == "turn" and grant["refresh"] == "snapshot", grant
assert all(not session["messages"] for session in sessions), sessions
assert json.loads(scopes_path.read_text()) == [selected]
print("SEND_TO_AI_ONE_TURN_GRANT_OK", "sent=no")
PY
kill "$PANEL_PID" 2>/dev/null || true
wait "$PANEL_PID" 2>/dev/null || true
PANEL_PID=""

if [[ "${EVERMORE_PHASE8_VERIFY_INTERNAL_DRAG:-}" == "1" ]]; then
  # Start a clean conversation in the existing drawer. The selected file
  # remains in the index, but the new session starts without its grant.
  swaymsg -s "$SWAYSOCK" seat seat0 cursor set 206 130 >/dev/null
  swaymsg -s "$SWAYSOCK" seat seat0 cursor press button1 >/dev/null
  sleep 0.1
  swaymsg -s "$SWAYSOCK" seat seat0 cursor release button1 >/dev/null
  sleep 0.4
  if [[ -n "${EVERMORE_PHASE8_DRAG_INITIAL_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE8_DRAG_INITIAL_SCREENSHOT"
  fi
  # Expand the open context tray and its selected-file list, then drag the
  # file label into the prompt using the VM's input-enabled headless seat.
  swaymsg -s "$SWAYSOCK" seat seat0 cursor set 80 510 >/dev/null
  swaymsg -s "$SWAYSOCK" seat seat0 cursor press button1 >/dev/null
  sleep 0.1
  swaymsg -s "$SWAYSOCK" seat seat0 cursor release button1 >/dev/null
  sleep 0.2
  swaymsg -s "$SWAYSOCK" seat seat0 cursor set 100 605 >/dev/null
  swaymsg -s "$SWAYSOCK" seat seat0 cursor press button1 >/dev/null
  sleep 0.1
  swaymsg -s "$SWAYSOCK" seat seat0 cursor release button1 >/dev/null
  sleep 0.2
  swaymsg -s "$SWAYSOCK" seat seat0 cursor set 90 663 >/dev/null
  swaymsg -s "$SWAYSOCK" seat seat0 cursor press button1 >/dev/null
  sleep 0.2
  swaymsg -s "$SWAYSOCK" seat seat0 cursor set 150 300 >/dev/null
  sleep 0.2
  swaymsg -s "$SWAYSOCK" seat seat0 cursor release button1 >/dev/null
  sleep 0.5
  if [[ -n "${EVERMORE_PHASE8_DRAG_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE8_DRAG_SCREENSHOT"
  fi
  python3 - "$XDG_DATA_HOME/evermore/ai-sessions.json" "$WORKDIR/selected.md" <<'PY'
import json, sys
from pathlib import Path
sessions=json.loads(Path(sys.argv[1]).read_text())
source="file:"+str(Path(sys.argv[2]).resolve())
assert sessions, sessions
latest=max(sessions, key=lambda session: session["updated_at"])
grants=[grant for grant in latest["context"] if grant["source"]==source]
assert len(grants)==1, latest
assert grants[0]["scope"]=="turn" and grants[0]["refresh"]=="snapshot", grants
assert not latest["messages"], latest
print("INTERNAL_FILE_DRAG_TO_PROMPT_ONE_TURN_GRANT_OK")
PY
  kill "$PANEL_PID" 2>/dev/null || true
  wait "$PANEL_PID" 2>/dev/null || true
  PANEL_PID=""
fi

launch_drawer "$WORKDIR/private.md"
python3 - "$XDG_CONFIG_HOME/evermore/index-files.json" "$WORKDIR/private.md" <<'PY'
import json
import sys
from pathlib import Path

scopes = json.loads(Path(sys.argv[1]).read_text())
private = str(Path(sys.argv[2]).resolve())
assert private not in scopes, (private, scopes)
print("SEND_TO_AI_NO_AI_REFUSAL_OK")
PY
