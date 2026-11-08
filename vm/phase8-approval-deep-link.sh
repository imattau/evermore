#!/usr/bin/env bash
# Exercise the SwayNC approval toast's Review action through the owner UI and
# the signed operation executor, using a disposable local relay and XDG tree.
set -euo pipefail

# Keep test identities in one explicit kernel session keyring across the
# separate keygen, signer, daemon, and panel processes.
if [[ "${EVERMORE_PHASE8_KEYRING_SESSION:-}" != "1" ]]; then
    command -v keyctl >/dev/null || { echo "keyctl is required for the Phase 8 approval test" >&2; exit 1; }
    exec keyctl session evermore-phase8-approval-test env EVERMORE_PHASE8_KEYRING_SESSION=1 bash "$0" "$@"
fi

REPO_ROOT="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${EVERMORE_PHASE8_BIN_DIR:-$REPO_ROOT/target/debug}"
RELAY_BIN="${EVERMORE_PHASE8_RELAY_BIN:-/tmp/evermore-relay-bin}"
RELAY_PORT="${EVERMORE_PHASE8_RELAY_PORT:-$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')}"
TEST_HOME="$(mktemp -d /tmp/evermore-phase8-approval.XXXXXX)"
XDG_RUNTIME_DIR="$TEST_HOME/runtime"
XDG_CONFIG_HOME="$TEST_HOME/config"
XDG_DATA_HOME="$TEST_HOME/data"
XDG_STATE_HOME="$TEST_HOME/state"
XDG_CACHE_HOME="$TEST_HOME/cache"
RELAY_URL="ws://127.0.0.1:$RELAY_PORT"
RUN_ID="$(date +%s)-$$"
OWNER_ID="00-p8-owner-$RUN_ID"
AGENT_ID="10-p8-agent-$RUN_ID"
SERVICE_ID="20-p8-service-$RUN_ID"
RELAY_PID=""
SIGNER_PID=""
SETTINGS_PID=""
OPERATIONS_PID=""
NOTIFY_PID=""
SWAY_PID=""
PANEL_PID=""

cleanup() {
    for pid in "$PANEL_PID" "$NOTIFY_PID" "$OPERATIONS_PID" "$SETTINGS_PID" "$SIGNER_PID" "$SWAY_PID" "$RELAY_PID"; do
        [[ -n "$pid" ]] || continue
        if kill -0 "$pid" 2>/dev/null; then
            kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
    if [[ "${EVERMORE_PHASE8_KEEP_TEST_HOME:-}" == "1" ]]; then
        echo "PRESERVED_TEST_HOME=$TEST_HOME"
    else
        rm -rf -- "$TEST_HOME"
    fi
}
trap cleanup EXIT INT TERM

for binary in evermore-signer evermore-settingsd evermore-operationsd evermore-notifyd evermore-panel; do
    [[ -x "$BIN_DIR/$binary" ]] || { echo "missing binary: $BIN_DIR/$binary" >&2; exit 1; }
done
[[ -x "$RELAY_BIN" ]] || { echo "missing local relay binary: $RELAY_BIN" >&2; exit 1; }
for command in sway swaymsg swaync swaync-client gdbus wtype grim python3 curl; do
    command -v "$command" >/dev/null || { echo "missing command: $command" >&2; exit 1; }
done

export PATH="$BIN_DIR:$PATH"
export XDG_RUNTIME_DIR XDG_CONFIG_HOME XDG_DATA_HOME XDG_STATE_HOME XDG_CACHE_HOME
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=/run/user/$(id -u)/bus}"
export EVERMORE_RELAY_URL="$RELAY_URL"
unset EVERMORE_CONTEXT_SOURCES EVERMORE_INDEX_FILES EVERMORE_SETTINGS_FILE
mkdir -m 0700 -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

HOME="$TEST_HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" XDG_DATA_HOME="$XDG_DATA_HOME" \
    "$REPO_ROOT/packaging/sway/install-desktop.sh" >/dev/null
printf 'set $mod Mod4\ninclude %s\ninclude %s\nfor_window [title="Review operation request"] floating enable\nfor_window [title="Review operation request"] resize set width 620 px height 460 px\nfor_window [title="Review operation request"] move position center\n' \
    "$XDG_CONFIG_HOME/sway/evermore-desktop.conf" \
    "$XDG_CONFIG_HOME/sway/evermore-waybar.conf" \
    > "$XDG_CONFIG_HOME/sway/config"

EVERMORE_RELAY_PORT="$RELAY_PORT" EVERMORE_RELAY_DATA_DIR="$TEST_HOME/relay-data" \
    "$RELAY_BIN" >"$TEST_HOME/relay.log" 2>&1 &
RELAY_PID=$!
for _ in $(seq 1 80); do
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
[[ -n "$OWNER_PUBKEY" && -n "$AGENT_PUBKEY" ]] || { echo "could not read test identity public keys" >&2; exit 1; }

"$BIN_DIR/evermore-signer" serve --socket "$XDG_RUNTIME_DIR/evermore/signer.sock" \
    >"$TEST_HOME/signer.log" 2>&1 &
SIGNER_PID=$!
for _ in $(seq 1 50); do
    [[ -S "$XDG_RUNTIME_DIR/evermore/signer.sock" ]] && break
    kill -0 "$SIGNER_PID" 2>/dev/null || { cat "$TEST_HOME/signer.log" >&2; exit 1; }
    sleep 0.1
done
[[ -S "$XDG_RUNTIME_DIR/evermore/signer.sock" ]] || { echo "test signer failed to start" >&2; exit 1; }

"$BIN_DIR/evermore-settingsd" --identity "$OWNER_ID" --relay "$RELAY_URL" \
    >"$TEST_HOME/settingsd.log" 2>&1 &
SETTINGS_PID=$!
"$BIN_DIR/evermore-operationsd" serve --owner-identity "$OWNER_ID" --service-identity "$SERVICE_ID" \
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
    echo "settings or operations daemon failed to create its socket" >&2
    cat "$TEST_HOME/settingsd.log" "$TEST_HOME/operationsd.log" >&2
    exit 1
}

"$BIN_DIR/evermore-operationsd" grant "$AGENT_PUBKEY" settings.set --identity "$OWNER_ID" \
    >"$TEST_HOME/grant.json"
GRANT_ACTIVE="$(python3 - "$TEST_HOME/grant.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
print("yes" if data.get("grant_published") else "no")
PY
)"
[[ "$GRANT_ACTIVE" == "yes" ]] || { cat "$TEST_HOME/grant.json" >&2; echo "owner grant was not acknowledged by the relay" >&2; exit 1; }

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

"$BIN_DIR/evermore-notifyd" --identity "$OWNER_ID" --local-relay "$RELAY_URL" \
    >"$TEST_HOME/notifyd.log" 2>&1 &
NOTIFY_PID=$!
for _ in $(seq 1 80); do
    swaync-client --count >/dev/null 2>&1 && break
    sleep 0.1
done
swaync-client --count >/dev/null 2>&1 || {
    cat "$TEST_HOME/sway.log" "$TEST_HOME/notifyd.log" >&2
    echo "SwayNC did not become available in the temporary session" >&2
    exit 1
}

if [[ "${EVERMORE_PHASE8_INBOX_TEST:-}" == "1" ]]; then
    # Seed an ordinary application toast before the approval deep link. Both
    # must appear together in SwayNC's history exactly once.
    gdbus call --session \
        --dest org.freedesktop.Notifications \
        --object-path /org/freedesktop/Notifications \
        --method org.freedesktop.Notifications.Notify \
        "Phase 8 test app" 0 "" "Local desktop event" "Inbox integration fixture" "[]" "{}" 5000 \
        >/dev/null
    for _ in $(seq 1 50); do
        NOTIFICATION_COUNT="$(swaync-client --count 2>/dev/null || true)"
        [[ "$NOTIFICATION_COUNT" =~ ^[0-9]+$ ]] && (( NOTIFICATION_COUNT == 1 )) && break
        sleep 0.1
    done
    [[ "${NOTIFICATION_COUNT:-0}" == "1" ]] || {
        echo "local desktop notification did not reach SwayNC exactly once" >&2
        exit 1
    }
fi

REQUEST_JSON="$("$BIN_DIR/evermore-operationsd" request settings.set \
    '{"domain":"theme","key":"mode","value":"dark"}' \
    --identity "$AGENT_ID" --owner-pubkey "$OWNER_PUBKEY")"
OPERATION_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["operation_id"])' <<<"$REQUEST_JSON")"
[[ -n "$OPERATION_ID" ]] || { echo "operation request did not return an ID" >&2; exit 1; }
echo "TEST_OPERATION_ID=$OPERATION_ID"

EXPECTED_NOTIFICATION_COUNT=1
if [[ "${EVERMORE_PHASE8_INBOX_TEST:-}" == "1" ]]; then
    EXPECTED_NOTIFICATION_COUNT=2
fi
for _ in $(seq 1 100); do
    NOTIFICATION_COUNT="$(swaync-client --count 2>/dev/null || true)"
    [[ "$NOTIFICATION_COUNT" =~ ^[0-9]+$ ]] && (( NOTIFICATION_COUNT == EXPECTED_NOTIFICATION_COUNT )) && break
    if ! kill -0 "$NOTIFY_PID" 2>/dev/null; then cat "$TEST_HOME/notifyd.log" >&2; exit 1; fi
    sleep 0.2
done
[[ "${NOTIFICATION_COUNT:-0}" =~ ^[0-9]+$ ]] && (( NOTIFICATION_COUNT == EXPECTED_NOTIFICATION_COUNT )) || {
    cat "$TEST_HOME/notifyd.log" >&2
    echo "expected ${EXPECTED_NOTIFICATION_COUNT} notification(s) in SwayNC, got ${NOTIFICATION_COUNT:-none}" >&2
    exit 1
}
echo "APPROVAL_NOTIFICATION_SHOWN_ONCE_OK"

# In inbox mode, open SwayNC's actual control center and verify showing its
# rows does not create extra notification records. Leave action verification
# to the dedicated single-approval deep-link path below.
if [[ "${EVERMORE_PHASE8_INBOX_TEST:-}" == "1" ]]; then
    swaync-client --open-panel
    sleep 0.3
    grim "$TEST_HOME/unified-inbox.png"
    swaync-client --close-panel
    [[ "$(swaync-client --count)" == "$EXPECTED_NOTIFICATION_COUNT" ]] || {
        echo "opening the SwayNC inbox changed or duplicated notification records" >&2
        exit 1
    }
    echo "UNIFIED_INBOX_LOCAL_AND_APPROVAL_ONCE_OK"
    exit 0
fi

# SwayNC action 0 is the approval notification's first registered action.
swaync-client --action 0
for _ in $(seq 1 80); do
    PANEL_PID="$(pgrep -n -x evermore-panel 2>/dev/null || true)"
    [[ -n "$PANEL_PID" ]] && break
    sleep 0.1
done
[[ -n "$PANEL_PID" ]] || {
    cat "$TEST_HOME/notifyd.log" "$TEST_HOME/sway.log" >&2
    echo "the notification's Review action did not open Evermore" >&2
    exit 1
}
sleep 1
grim "$TEST_HOME/approval-review.png"
TREE="$(swaymsg -s "$SWAY_SOCKET" -t get_tree -r)"
python3 - "$TREE" <<'PY'
import json, sys
tree = json.loads(sys.argv[1])
def walk(node):
    yield node
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        yield from walk(child)
views = [node for node in walk(tree) if node.get("name") == "Review operation request"]
assert views, "review window did not appear"
PY
echo "APPROVAL_REVIEW_ACTION_OPENS_REVIEW_WINDOW_OK"

if [[ "${EVERMORE_PHASE8_STOP_AFTER_REVIEW_OPEN:-}" == "1" ]]; then
    echo "TEST_STOPPED_AFTER_REVIEW_OPEN=1"
    exit 0
fi

# The test owner is lexically first among the test identities, so the
# approval view preselects that identity. Click its explicit Approve control.
# The fixed 1280x720 headless output centers the 620x460 review window at
# (330, 130); the Approve button is at (365, 440).
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set 365 440 >/dev/null
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
sleep 0.1
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null
for _ in $(seq 1 80); do
    STATUS_JSON="$("$BIN_DIR/evermore-operationsd" status "$OPERATION_ID" 2>/dev/null || true)"
    STATUS="$(python3 -c 'import json,sys; print(json.load(sys.stdin).get("status", ""))' <<<"$STATUS_JSON" 2>/dev/null || true)"
    [[ "$STATUS" == "succeeded" ]] && break
    sleep 0.25
done
[[ "${STATUS:-}" == "succeeded" ]] || {
    grim "$TEST_HOME/approval-decision-failure.png" || true
    cat "$TEST_HOME/operationsd.log" "$TEST_HOME/settingsd.log" >&2
    echo "the explicit owner decision did not complete settings.set (status=${STATUS:-unknown})" >&2
    exit 1
}
echo "APPROVAL_DEEP_LINK_MATCHES_REQUEST_AND_OWNER_APPROVES_OK"
