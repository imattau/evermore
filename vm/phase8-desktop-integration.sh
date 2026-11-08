#!/usr/bin/env bash
# Run the existing Send to AI and focused-output gates against one disposable
# headless Sway session, so they do not depend on the user's live desktop.
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_HOME="$(mktemp -d /tmp/evermore-phase8-desktop.XXXXXX)"
XDG_RUNTIME_DIR="$TEST_HOME/runtime"
XDG_CONFIG_HOME="$TEST_HOME/config"
XDG_DATA_HOME="$TEST_HOME/data"
XDG_STATE_HOME="$TEST_HOME/state"
XDG_CACHE_HOME="$TEST_HOME/cache"
SWAY_PID=""

cleanup() {
    if [[ -n "$SWAY_PID" ]] && kill -0 "$SWAY_PID" 2>/dev/null; then
        kill -TERM -- "-$SWAY_PID" 2>/dev/null || kill -TERM "$SWAY_PID" 2>/dev/null || true
        wait "$SWAY_PID" 2>/dev/null || true
    fi
    rm -rf -- "$TEST_HOME"
}
trap cleanup EXIT INT TERM

for binary in sway swaymsg; do
    command -v "$binary" >/dev/null || { echo "missing required command: $binary" >&2; exit 1; }
done
for binary in evermore-panel evermore-indexd; do
    [[ -x "$REPO_ROOT/target/debug/$binary" ]] || { echo "missing required binary: $binary" >&2; exit 1; }
done

export XDG_RUNTIME_DIR XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME
mkdir -m 0700 -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"
HOME="$TEST_HOME" "$REPO_ROOT/packaging/sway/install-desktop.sh" >/dev/null
printf 'set $mod Mod4\ninclude %s\ninclude %s\n' \
    "$XDG_CONFIG_HOME/sway/evermore-desktop.conf" \
    "$XDG_CONFIG_HOME/sway/evermore-waybar.conf" \
    > "$XDG_CONFIG_HOME/sway/config"

setsid env \
    HOME="$TEST_HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" XDG_DATA_HOME="$XDG_DATA_HOME" \
    XDG_STATE_HOME="$XDG_STATE_HOME" XDG_CACHE_HOME="$XDG_CACHE_HOME" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
    WLR_BACKENDS=headless,libinput WLR_HEADLESS_OUTPUTS=1 WLR_RENDERER=pixman \
    sway -c "$XDG_CONFIG_HOME/sway/config" -d >"$TEST_HOME/sway.log" 2>&1 </dev/null &
SWAY_PID=$!
SWAYSOCK="$XDG_RUNTIME_DIR/sway-ipc.$(id -u).$SWAY_PID.sock"
for _ in $(seq 1 80); do
    if swaymsg -s "$SWAYSOCK" -t get_outputs -r >/dev/null 2>&1; then break; fi
    kill -0 "$SWAY_PID" 2>/dev/null || { cat "$TEST_HOME/sway.log" >&2; exit 1; }
    sleep 0.1
done
[[ -S "$SWAYSOCK" ]] || { cat "$TEST_HOME/sway.log" >&2; echo "temporary Sway failed to start" >&2; exit 1; }
WAYLAND_DISPLAY="$(sed -n "s/.*Running compositor on wayland display '\([^']*\)'.*/\1/p" "$TEST_HOME/sway.log" | tail -1)"
[[ -n "$WAYLAND_DISPLAY" ]] || { echo "could not determine temporary Wayland display" >&2; exit 1; }
export WAYLAND_DISPLAY SWAYSOCK

bash "$REPO_ROOT/vm/phase8-send-to-ai.sh"
bash "$REPO_ROOT/vm/phase8-ai-window-focus.sh"
bash "$REPO_ROOT/vm/phase8-ai-output-focus.sh"
echo "PHASE8_SEND_TO_AI_AND_OUTPUT_FOCUS_OK"
