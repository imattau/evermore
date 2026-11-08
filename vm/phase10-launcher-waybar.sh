#!/usr/bin/env bash
# Test the app-grid launcher and packaged Waybar in an isolated input-enabled
# Sway session. This leaves the VM's regular desktop session untouched.
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_HOME="$(mktemp -d /tmp/evermore-phase10-desktop.XXXXXX)"
TEST_RUNTIME="$TEST_HOME/runtime"
SWAY_PID=""

cleanup() {
    if [[ -n "$SWAY_PID" ]] && kill -0 "$SWAY_PID" 2>/dev/null; then
        kill -TERM -- "-$SWAY_PID" 2>/dev/null || kill -TERM "$SWAY_PID" 2>/dev/null || true
        wait "$SWAY_PID" 2>/dev/null || true
    fi
    if [[ "${EVERMORE_PHASE10_KEEP_TEST_HOME:-}" == "1" ]]; then
        echo "PRESERVED_TEST_HOME=$TEST_HOME"
    else
        rm -rf -- "$TEST_HOME"
    fi
}
trap cleanup EXIT INT TERM

for binary in sway waybar wofi fuzzel wtype grim; do
    command -v "$binary" >/dev/null || { echo "$binary is required in the VM" >&2; exit 1; }
done
mkdir -m 0700 -p "$TEST_RUNTIME"

HOME="$TEST_HOME" \
XDG_CONFIG_HOME="$TEST_HOME/config" \
XDG_DATA_HOME="$TEST_HOME/data" \
"$REPO_ROOT/packaging/sway/install-desktop.sh" >/dev/null

printf 'set $mod Mod4\ninclude %s\ninclude %s\n' \
    "$TEST_HOME/config/sway/evermore-desktop.conf" \
    "$TEST_HOME/config/sway/evermore-waybar.conf" \
    > "$TEST_HOME/config/sway/config"

setsid env \
    HOME="$TEST_HOME" \
    XDG_CONFIG_HOME="$TEST_HOME/config" \
    XDG_DATA_HOME="$TEST_HOME/data" \
    XDG_RUNTIME_DIR="$TEST_RUNTIME" \
    WLR_BACKENDS=headless,libinput \
    WLR_HEADLESS_OUTPUTS=1 \
    WLR_RENDERER=pixman \
    sway -c "$TEST_HOME/config/sway/config" -d \
    > "$TEST_HOME/sway.log" 2>&1 </dev/null &
SWAY_PID=$!

SWAY_SOCKET="$TEST_RUNTIME/sway-ipc.$(id -u).$SWAY_PID.sock"
for _ in $(seq 1 40); do
    if swaymsg -s "$SWAY_SOCKET" -t get_seats -r > "$TEST_HOME/seats.json" 2>/dev/null; then
        break
    fi
    if ! kill -0 "$SWAY_PID" 2>/dev/null; then
        cat "$TEST_HOME/sway.log" >&2
        exit 1
    fi
    sleep 0.25
done
grep -q 'AT_Translated_Set_2_keyboard' "$TEST_HOME/seats.json" || {
    echo "temporary Sway session did not attach the VM keyboard" >&2
    cat "$TEST_HOME/seats.json" >&2
    exit 1
}

WAYLAND_DISPLAY="$(sed -n "s/.*Running compositor on wayland display '\([^']*\)'.*/\1/p" \
    "$TEST_HOME/sway.log" | tail -1)"
[[ -n "$WAYLAND_DISPLAY" ]] || { echo "could not determine Wayland display" >&2; exit 1; }
export XDG_RUNTIME_DIR="$TEST_RUNTIME" WAYLAND_DISPLAY

for _ in $(seq 1 40); do
    grep -q "Using configuration file $TEST_HOME/config/waybar/config" "$TEST_HOME/sway.log" && break
    sleep 0.1
done
grep -q "Using configuration file $TEST_HOME/config/waybar/config" "$TEST_HOME/sway.log" || { echo "Sway did not start Waybar from the isolated config" >&2; exit 1; }
sleep 0.5
if [[ -n "${EVERMORE_PHASE10_WAYBAR_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE10_WAYBAR_SCREENSHOT"
fi
echo "WAYBAR_STARTED_WITH_PACKAGED_CONFIG_OK"

FOOT_BASELINE="$(pgrep -x foot 2>/dev/null | wc -l || true)"
wtype -M logo -k space -m logo
for _ in $(seq 1 50); do
    pgrep -x wofi >/dev/null && break
    sleep 0.1
done
pgrep -x wofi >/dev/null || { echo "Super+Space did not open Wofi" >&2; exit 1; }
sleep 0.5
if [[ -n "${EVERMORE_PHASE10_GRID_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE10_GRID_SCREENSHOT"
fi
echo "WOFI_EMPTY_QUERY_APP_GRID_OPENED_OK"

# The first printable character after the hotkey must reach the search field.
wtype foot
sleep 0.5
if [[ -n "${EVERMORE_PHASE10_FILTER_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE10_FILTER_SCREENSHOT"
fi
wtype -k Return
for _ in $(seq 1 50); do
    ! pgrep -x wofi >/dev/null && break
    sleep 0.1
done
! pgrep -x wofi >/dev/null || { echo "Enter did not close Wofi" >&2; exit 1; }
FOOT_AFTER="$(pgrep -x foot 2>/dev/null | wc -l || true)"
(( FOOT_AFTER > FOOT_BASELINE )) || { echo "Enter did not launch filtered Foot app" >&2; exit 1; }
echo "WOFI_FIRST_KEY_FUZZY_FILTER_AND_LAUNCH_OK"

wtype -M logo -k space -m logo
for _ in $(seq 1 50); do
    pgrep -x wofi >/dev/null && break
    sleep 0.1
done
pgrep -x wofi >/dev/null || { echo "second Super+Space did not open Wofi" >&2; exit 1; }
sleep 0.5
wtype -k Escape
for _ in $(seq 1 50); do
    ! pgrep -x wofi >/dev/null && break
    sleep 0.1
done
! pgrep -x wofi >/dev/null || { grim "$TEST_HOME/escape-failure.png" || true; swaymsg -s "$SWAY_SOCKET" -t get_tree -r > "$TEST_HOME/escape-tree.json" || true; echo "Escape did not dismiss Wofi" >&2; exit 1; }
echo "WOFI_ESCAPE_DISMISS_OK"

wtype -M logo -k space -m logo
for _ in $(seq 1 50); do
    pgrep -x wofi >/dev/null && break
    sleep 0.1
done
pgrep -x wofi >/dev/null || { echo "third Super+Space did not open Wofi" >&2; exit 1; }
sleep 0.5
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set 40 700 >/dev/null
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
sleep 0.1
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null
for _ in $(seq 1 50); do
    ! pgrep -x wofi >/dev/null && break
    sleep 0.1
done
! pgrep -x wofi >/dev/null || { echo "outside click did not dismiss Wofi" >&2; exit 1; }
echo "WOFI_OUTSIDE_CLICK_DISMISS_OK"
