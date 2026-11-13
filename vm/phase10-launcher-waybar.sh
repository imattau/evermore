#!/usr/bin/env bash
# Test the eGUI app launcher and packaged Waybar in an isolated
# input-enabled Sway session. This leaves the VM's regular desktop untouched.
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

for binary in sway waybar wtype grim python3; do
    command -v "$binary" >/dev/null || { echo "$binary is required in the VM" >&2; exit 1; }
done
LAUNCHER_BIN="${EVERMORE_LAUNCHER_BIN:-$REPO_ROOT/target/debug/evermore-launcher}"
[[ -x "$LAUNCHER_BIN" ]] || {
    echo "build evermore-launcher in the VM first: cargo build -p evermore-launcher" >&2
    exit 1
}
export PATH="$(dirname -- "$LAUNCHER_BIN"):$PATH"
mkdir -m 0700 -p "$TEST_RUNTIME"

HOME="$TEST_HOME" \
XDG_CONFIG_HOME="$TEST_HOME/config" \
XDG_DATA_HOME="$TEST_HOME/data" \
XDG_STATE_HOME="$TEST_HOME/state" \
"$REPO_ROOT/packaging/sway/install-desktop.sh" >/dev/null

mkdir -p "$TEST_HOME/data/applications" "$TEST_HOME/bin"
FIXTURE="$TEST_HOME/bin/fixture launcher"
cat > "$FIXTURE" <<'EOF'
#!/usr/bin/env sh
printf '%s\n' "$1" >> "$EVERMORE_LAUNCHER_MARKER"
EOF
chmod 0755 "$FIXTURE"
cat > "$TEST_HOME/data/applications/org.evermore.MouseFixture.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=000 Mouse Fixture
Comment=Test mouse launching
Exec="$FIXTURE" mouse
Terminal=false
EOF
cat > "$TEST_HOME/data/applications/org.evermore.KeyboardFixture.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Keyboard Fixture
Comment=Test keyboard launching
Keywords=fixture;test;
Exec="$FIXTURE" keyboard
Terminal=false
EOF

printf 'set $mod Mod4\ninclude %s\ninclude %s\n' \
    "$TEST_HOME/config/sway/evermore-desktop.conf" \
    "$TEST_HOME/config/sway/evermore-waybar.conf" \
    > "$TEST_HOME/config/sway/config"

setsid env \
    HOME="$TEST_HOME" \
    XDG_CONFIG_HOME="$TEST_HOME/config" \
    XDG_DATA_HOME="$TEST_HOME/data" \
    XDG_STATE_HOME="$TEST_HOME/state" \
    XDG_DATA_DIRS=/usr/local/share:/usr/share \
    EVERMORE_LAUNCHER_MARKER="$TEST_HOME/launches.log" \
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
export HOME="$TEST_HOME" XDG_CONFIG_HOME="$TEST_HOME/config" XDG_DATA_HOME="$TEST_HOME/data"
export XDG_STATE_HOME="$TEST_HOME/state" XDG_DATA_DIRS=/usr/local/share:/usr/share
export EVERMORE_LAUNCHER_MARKER="$TEST_HOME/launches.log"

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

! grep -q 'wofi' "$TEST_HOME/config/sway/evermore-desktop.conf" || {
    echo "Sway config still binds Wofi" >&2
    exit 1
}

# The first fixture sorts first in the all-apps grid, so a pointer click
# inside its tile exercises mouse launch without depending on the host app set.
wtype -M logo -k space -m logo
for _ in $(seq 1 50); do
    pgrep -f '(^|/)evermore-launcher$' >/dev/null && break
    sleep 0.1
done
pgrep -f '(^|/)evermore-launcher$' >/dev/null || { echo "Super+Space did not open evermore-launcher" >&2; cat "$TEST_HOME/sway.log" >&2; exit 1; }
sleep 0.5
if [[ -n "${EVERMORE_PHASE10_GRID_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE10_GRID_SCREENSHOT"
fi
echo "EGUI_EMPTY_QUERY_APP_GRID_OPENED_OK"

wtype "Mouse Fixture"
sleep 0.3

swaymsg -s "$SWAY_SOCKET" -t get_tree -r > "$TEST_HOME/tree.json"
python3 - "$TEST_HOME/tree.json" > "$TEST_HOME/launcher-rect.txt" <<'PY'
import json, sys
tree = json.load(open(sys.argv[1]))
def walk(node):
    if node.get("app_id") == "org.evermore.Launcher":
        rect = node["rect"]
        print(rect["x"], rect["y"], rect["width"], rect["height"])
        return True
    return any(walk(child) for child in node.get("nodes", []) + node.get("floating_nodes", []))
if not walk(tree):
    raise SystemExit("launcher app_id not found in Sway tree")
PY
read -r LAUNCH_X LAUNCH_Y LAUNCH_WIDTH LAUNCH_HEIGHT < "$TEST_HOME/launcher-rect.txt"
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set "$((LAUNCH_X + 100))" "$((LAUNCH_Y + 205))" >/dev/null
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
sleep 0.1
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null
for _ in $(seq 1 50); do
    ! pgrep -f '(^|/)evermore-launcher$' >/dev/null && break
    sleep 0.1
done
! pgrep -f '(^|/)evermore-launcher$' >/dev/null || { echo "mouse click did not launch the first fixture" >&2; exit 1; }
grep -q '^mouse$' "$TEST_HOME/launches.log" || { echo "mouse launch fixture did not run" >&2; exit 1; }
echo "EGUI_MOUSE_TILE_LAUNCH_OK"

# The first printable character after the hotkey must reach the search field.
wtype -M logo -k space -m logo
for _ in $(seq 1 50); do
    pgrep -f '(^|/)evermore-launcher$' >/dev/null && break
    sleep 0.1
done
pgrep -f '(^|/)evermore-launcher$' >/dev/null || { echo "launcher did not reopen" >&2; exit 1; }
# The process exists before its Wayland surface is mapped and focused. Wait
# until Sway reports the mapped launcher before injecting the first character.
for _ in $(seq 1 20); do
    swaymsg -s "$SWAY_SOCKET" -t get_tree -r > "$TEST_HOME/tree.json"
    if python3 - "$TEST_HOME/tree.json" <<'PY'
import json, sys
tree = json.load(open(sys.argv[1]))
def walk(node):
    if node.get("app_id") == "org.evermore.Launcher":
        return bool(node.get("focused"))
    return any(walk(child) for child in node.get("nodes", []) + node.get("floating_nodes", []))
raise SystemExit(0 if walk(tree) else 1)
PY
    then
        break
    fi
    sleep 0.1
done
wtype "Keyboard Fixture"
sleep 0.5
if [[ -n "${EVERMORE_PHASE10_FILTER_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE10_FILTER_SCREENSHOT"
fi
wtype -M ctrl -k d -m ctrl
sleep 0.2
grep -q 'org.evermore.KeyboardFixture' "$TEST_HOME/config/evermore/launcher.json" || {
    echo "Ctrl+D did not persist the selected favorite" >&2
    exit 1
}
wtype -k Return
for _ in $(seq 1 50); do
    ! pgrep -f '(^|/)evermore-launcher$' >/dev/null && break
    sleep 0.1
done
! pgrep -f '(^|/)evermore-launcher$' >/dev/null || { echo "Enter did not close launcher" >&2; exit 1; }
grep -q '^keyboard$' "$TEST_HOME/launches.log" || { echo "Enter did not launch filtered app" >&2; exit 1; }
grep -q 'org.evermore.KeyboardFixture' "$TEST_HOME/state/evermore/launcher-history.json" || {
    echo "successful launch did not persist recent history" >&2
    exit 1
}
echo "EGUI_FUZZY_SEARCH_FAVORITE_AND_KEYBOARD_LAUNCH_OK"

# A query with no match remains an understandable, dismissible launcher state.
wtype -M logo -k space -m logo
for _ in $(seq 1 50); do
    pgrep -f '(^|/)evermore-launcher$' >/dev/null && break
    sleep 0.1
done
pgrep -f '(^|/)evermore-launcher$' >/dev/null || { echo "launcher did not reopen for no-results check" >&2; exit 1; }
wtype notarealappquery
sleep 0.4
if [[ -n "${EVERMORE_PHASE10_FILTER_SCREENSHOT:-}" ]]; then
    grim "$EVERMORE_PHASE10_FILTER_SCREENSHOT"
fi
pgrep -f '(^|/)evermore-launcher$' >/dev/null || { echo "no-results query closed launcher unexpectedly" >&2; exit 1; }
wtype -k Escape
for _ in $(seq 1 50); do
    ! pgrep -f '(^|/)evermore-launcher$' >/dev/null && break
    sleep 0.1
done
! pgrep -f '(^|/)evermore-launcher$' >/dev/null || { grim "$TEST_HOME/escape-failure.png" || true; echo "Escape did not dismiss launcher" >&2; exit 1; }
echo "EGUI_NO_RESULTS_AND_ESCAPE_DISMISS_OK"

# Outside click must remove focus and dismiss the regular floating window.
wtype -M logo -k space -m logo
for _ in $(seq 1 50); do
    pgrep -f '(^|/)evermore-launcher$' >/dev/null && break
    sleep 0.1
done
pgrep -f '(^|/)evermore-launcher$' >/dev/null || { echo "launcher did not reopen for focus-loss check" >&2; exit 1; }
sleep 0.5
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set 40 700 >/dev/null
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
sleep 0.1
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null
for _ in $(seq 1 50); do
    ! pgrep -f '(^|/)evermore-launcher$' >/dev/null && break
    sleep 0.1
done
! pgrep -f '(^|/)evermore-launcher$' >/dev/null || { echo "outside click did not dismiss launcher" >&2; exit 1; }
echo "EGUI_FOCUS_LOSS_DISMISS_OK"
