#!/usr/bin/env bash
# Validate Wofi's keyboard path in an isolated Sway session with libinput.
# The normal build VM starts a headless-only compositor; adding libinput here
# attaches its existing AT keyboard for a realistic launcher focus test.
set -euo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_HOME="$(mktemp -d /tmp/evermore-phase8-launcher.XXXXXX)"
TEST_RUNTIME="$TEST_HOME/runtime"
SWAY_PID=""
INDEX_PID=""
INFER_PID=""

cleanup() {
    if [[ -n "$SWAY_PID" ]] && kill -0 "$SWAY_PID" 2>/dev/null; then
        kill -TERM -- "-$SWAY_PID" 2>/dev/null || kill -TERM "$SWAY_PID" 2>/dev/null || true
        wait "$SWAY_PID" 2>/dev/null || true
    fi
    if [[ -n "$INDEX_PID" ]]; then
        kill "$INDEX_PID" 2>/dev/null || true
        wait "$INDEX_PID" 2>/dev/null || true
    fi
    if [[ -n "$INFER_PID" ]]; then
        kill "$INFER_PID" 2>/dev/null || true
        wait "$INFER_PID" 2>/dev/null || true
    fi
    if [[ "${EVERMORE_PHASE8_KEEP_TEST_HOME:-}" == "1" ]]; then
        echo "PRESERVED_TEST_HOME=$TEST_HOME"
    else
        rm -rf -- "$TEST_HOME"
    fi
}
trap cleanup EXIT INT TERM

command -v sway >/dev/null
command -v fuzzel >/dev/null
command -v wofi >/dev/null
command -v wtype >/dev/null || {
    echo "wtype is required in the VM for keyboard-event injection" >&2
    exit 1
}
command -v grim >/dev/null
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
    PATH="$REPO_ROOT/target/debug:${PATH:-/usr/local/bin:/usr/bin:/bin}" \
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
[[ -n "$WAYLAND_DISPLAY" ]] || {
    echo "could not determine temporary Sway Wayland display" >&2
    exit 1
}
export XDG_RUNTIME_DIR="$TEST_RUNTIME"
export WAYLAND_DISPLAY

if [[ -n "${EVERMORE_PHASE8_WAYBAR_SCREENSHOT:-}" ]]; then
    for _ in $(seq 1 40); do
        pgrep -x waybar >/dev/null && break
        sleep 0.1
    done
    pgrep -x waybar >/dev/null || {
        echo "Sway did not start Waybar" >&2
        cat "$TEST_HOME/sway.log" >&2
        exit 1
    }
    grim "$EVERMORE_PHASE8_WAYBAR_SCREENSHOT"
    echo "WAYBAR_STARTED_AND_SCREENSHOT_OK"
fi

FOOT_BASELINE="$(pgrep -x foot 2>/dev/null | wc -l || true)"
wtype -M logo -k space -m logo
for _ in $(seq 1 40); do
    pgrep -x wofi >/dev/null && break
    sleep 0.1
done
pgrep -x wofi >/dev/null || { echo "Super+Space did not open Wofi" >&2; exit 1; }
sleep 0.5
wtype foot
sleep 0.5
grim "$TEST_HOME/filtered.png"
if [[ -n "${EVERMORE_PHASE8_SCREENSHOT:-}" ]]; then
    cp -- "$TEST_HOME/filtered.png" "$EVERMORE_PHASE8_SCREENSHOT"
fi
wtype -k Return
for _ in $(seq 1 40); do
    ! pgrep -x wofi >/dev/null && break
    sleep 0.1
done
! pgrep -x wofi >/dev/null || { echo "Enter did not close Wofi" >&2; exit 1; }
FOOT_AFTER="$(pgrep -x foot 2>/dev/null | wc -l || true)"
(( FOOT_AFTER > FOOT_BASELINE )) || {
    echo "Enter did not launch the filtered Foot app" >&2
    exit 1
}
echo "WOFI_HOTKEY_TEXT_FILTER_ENTER_OK"

wtype -M logo -k space -m logo
for _ in $(seq 1 40); do
    pgrep -x wofi >/dev/null && break
    sleep 0.1
done
pgrep -x wofi >/dev/null || { echo "second Super+Space did not open Wofi" >&2; exit 1; }
sleep 0.5
wtype -k Escape
for _ in $(seq 1 40); do
    ! pgrep -x wofi >/dev/null && break
    sleep 0.1
done
! pgrep -x wofi >/dev/null || { echo "Escape did not dismiss Wofi" >&2; exit 1; }
echo "WOFI_ESCAPE_DISMISS_OK"

wtype -M logo -k space -m logo
for _ in $(seq 1 40); do
    pgrep -x wofi >/dev/null && break
    sleep 0.1
done
pgrep -x wofi >/dev/null || { echo "third Super+Space did not open Wofi" >&2; exit 1; }
sleep 0.5
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor set 40 700 >/dev/null
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor press button1 >/dev/null
sleep 0.1
swaymsg -s "$SWAY_SOCKET" seat seat0 cursor release button1 >/dev/null
for _ in $(seq 1 40); do
    ! pgrep -x wofi >/dev/null && break
    sleep 0.1
done
! pgrep -x wofi >/dev/null || {
    grim "$TEST_HOME/focus-loss-failure.png" || true
    echo "clicking outside Wofi did not dismiss it" >&2
    exit 1
}
echo "WOFI_OUTSIDE_CLICK_DISMISS_OK"

if [[ -x "$REPO_ROOT/target/debug/evermore-panel" ]] && command -v python3 >/dev/null; then
    INDEX_DIR="$TEST_RUNTIME/evermore"
    INDEX_SOCKET="$INDEX_DIR/ai-indexd.sock"
    INDEX_LOG="$TEST_HOME/index-requests.log"
    TEST_FILE="$TEST_HOME/phase8-semantic-note.md"
    mkdir -m 0700 -p "$INDEX_DIR"
    printf '%s\n' 'The community telescope is stored in the north equipment shed.' > "$TEST_FILE"
    python3 - "$INDEX_SOCKET" "$INDEX_LOG" "$TEST_FILE" <<'PY' &
import json
import os
import socket
import sys

socket_path, request_log, result_path = sys.argv[1:]
server = socket.socket(socket.AF_UNIX)
server.bind(socket_path)
os.chmod(socket_path, 0o600)
server.listen(8)
with open(request_log, "a", encoding="utf-8") as log:
    while True:
        connection, _ = server.accept()
        with connection:
            request = json.loads(connection.makefile().readline())
            method = request["method"]
            log.write(method + "\n")
            log.flush()
            if method == "list_scopes":
                response = {"event": "scopes", "scopes": [{"id": "phase8-scope", "path": result_path}]}
            elif method == "search":
                response = {"event": "results", "query": request["params"]["query"], "results": [{
                    "scope_id": "phase8-scope",
                    "path": result_path,
                    "excerpt": "The telescope is stored in the north equipment shed.",
                    "score": 0.99,
                }]}
            else:
                response = {"event": "error", "message": "unexpected request"}
            connection.sendall((json.dumps(response) + "\n").encode())
PY
    INDEX_PID=$!
    for _ in $(seq 1 40); do
        [[ -S "$INDEX_SOCKET" ]] && break
        sleep 0.1
    done
    [[ -S "$INDEX_SOCKET" ]] || { echo "fake index socket did not start" >&2; exit 1; }

    if [[ "${EVERMORE_PHASE8_AI_FALLBACK_TEST:-}" == "1" || "${EVERMORE_PHASE8_AI_FALLBACK_CANCEL_TEST:-}" == "1" ]]; then
        INFER_SOCKET="$INDEX_DIR/ai-inferd.sock"
        INFER_LOG="$TEST_HOME/infer-requests.jsonl"
        python3 - "$INFER_SOCKET" "$INFER_LOG" "${EVERMORE_PHASE8_AI_FALLBACK_CANCEL_TEST:-0}" <<'PY' &
import json, os, socket, sys
socket_path, log_path, cancel_mode=sys.argv[1:]
server=socket.socket(socket.AF_UNIX)
server.bind(socket_path)
os.chmod(socket_path, 0o600)
server.listen(4)
while True:
    connection,_=server.accept()
    with connection:
        request=json.loads(connection.makefile().readline())
        method=request["method"]
        if method=="list_models":
            response={"event":"models","models":[{
                "name":"evermore/test-chat:1.0","loaded":True,"capabilities":["chat"]
            }]}
            connection.sendall((json.dumps(response)+"\n").encode())
        elif method=="chat":
            with open(log_path,"a",encoding="utf-8") as log:
                log.write(json.dumps(request)+"\n")
            params=request["params"]
            request_id=params["request_id"]
            events=[
                {"event":"started","request_id":request_id,"model":params["model"],"backend_requested":"auto"},
                {"event":"token","request_id":request_id,"text":"Try the north equipment shed."},
            ]
            if cancel_mode == "1":
                connection.sendall(("".join(json.dumps(event)+"\n" for event in events)).encode())
                while connection.recv(1):
                    pass
                with open(log_path,"a",encoding="utf-8") as log:
                    log.write("CANCELLED_BY_LAUNCHER\n")
            else:
                events.append({"event":"completed","request_id":request_id,"backend_used":"auto","prompt_tokens":10,"completion_tokens":7,"total_duration_ns":1000000})
                connection.sendall(("".join(json.dumps(event)+"\n" for event in events)).encode())
        else:
            connection.sendall(b'{"event":"error","message":"unexpected request"}\n')
PY
        INFER_PID=$!
        for _ in $(seq 1 40); do
            [[ -S "$INFER_SOCKET" ]] && break
            sleep 0.1
        done
        [[ -S "$INFER_SOCKET" ]] || { echo "fake inference socket did not start" >&2; exit 1; }
    fi

    wtype -M logo -M shift -k f -m shift -m logo
    for _ in $(seq 1 40); do
        pgrep -x fuzzel >/dev/null && break
        sleep 0.1
    done
    pgrep -x fuzzel >/dev/null || { echo "Super+Shift+F did not open file search" >&2; exit 1; }
    wtype 'Where is the telescope stored?'
    wtype -k Return
    sleep 1
    if ! pgrep -x fuzzel >/dev/null; then
        grim "$TEST_HOME/semantic-failure.png" || true
        echo "semantic result picker did not open" >&2
        cat "$INDEX_LOG" >&2 || true
        cat "$INFER_LOG" >&2 2>/dev/null || true
        tail -60 "$TEST_HOME/sway.log" >&2 || true
        exit 1
    fi
    grep -q '^search$' "$INDEX_LOG" || { echo "typed no-match query did not reach semantic search" >&2; exit 1; }
    grim "$TEST_HOME/semantic-results.png"
    if [[ -n "${EVERMORE_PHASE8_SEMANTIC_SCREENSHOT:-}" ]]; then
        cp -- "$TEST_HOME/semantic-results.png" "$EVERMORE_PHASE8_SEMANTIC_SCREENSHOT"
    fi
    if [[ "${EVERMORE_PHASE8_AI_FALLBACK_CANCEL_TEST:-}" == "1" ]]; then
        wtype -k Escape
        for _ in $(seq 1 40); do
            grep -q 'CANCELLED_BY_LAUNCHER' "$INFER_LOG" 2>/dev/null && break
            sleep 0.1
        done
        grep -q 'CANCELLED_BY_LAUNCHER' "$INFER_LOG" || {
            echo "Escape did not cancel the in-progress warm suggestion" >&2
            exit 1
        }
        echo "WARM_AI_FALLBACK_ESCAPE_CANCELLATION_OK"
    elif [[ "${EVERMORE_PHASE8_AI_FALLBACK_TEST:-}" == "1" ]]; then
        wtype 'Local suggestion'
        wtype -k Return
        for _ in $(seq 1 40); do
            [[ "$(wl-paste --type text/plain --no-newline 2>/dev/null || true)" == "Try the north equipment shed." ]] && break
            sleep 0.1
        done
        [[ "$(wl-paste --type text/plain --no-newline 2>/dev/null || true)" == "Try the north equipment shed." ]] || {
            echo "warm local suggestion was not copied from the distinct launcher row" >&2
            exit 1
        }
        echo "WARM_AI_FALLBACK_AFTER_SEMANTIC_RESULTS_OK"
    else
        wtype -k Escape
    fi
    if [[ "${EVERMORE_PHASE8_AI_FALLBACK_TEST:-}" == "1" || "${EVERMORE_PHASE8_AI_FALLBACK_CANCEL_TEST:-}" == "1" ]]; then
        python3 - "$INFER_LOG" <<'PY'
import json,sys
requests=[json.loads(line) for line in open(sys.argv[1],encoding="utf-8") if line.startswith("{")]
assert len(requests)==1, requests
params=requests[0]["params"]
assert params.get("warm_only") is True, params
assert params["messages"][-1]["content"]=="Where is the telescope stored?", params
print("WARM_ONLY_OPEN_ENDED_REQUEST_OK")
PY
    fi
    for _ in $(seq 1 40); do
        ! pgrep -x fuzzel >/dev/null && break
        sleep 0.1
    done
    ! pgrep -x fuzzel >/dev/null || { echo "Escape did not close semantic picker" >&2; exit 1; }
    echo "FUZZEL_DETERMINISTIC_TO_SEMANTIC_PICKER_OK"
fi

if [[ -x "$REPO_ROOT/target/debug/evermore-panel" ]] && command -v wl-copy >/dev/null; then
    old_panel_count="$(pgrep -x evermore-panel 2>/dev/null | wc -l || true)"
    printf 'PHASE8_CLIPBOARD_CAPTURE_CHECK' | wl-copy --type text/plain
    wtype -M logo -M shift -k v -m shift -m logo
    for _ in $(seq 1 40); do
        current_panel_count="$(pgrep -x evermore-panel 2>/dev/null | wc -l || true)"
        (( current_panel_count > old_panel_count )) && break
        sleep 0.1
    done
    (( current_panel_count > old_panel_count )) || {
        echo "Super+Shift+V did not launch Evermore AI" >&2
        exit 1
    }
    sleep 1
    current_panel_count="$(pgrep -x evermore-panel 2>/dev/null | wc -l || true)"
    (( current_panel_count > old_panel_count )) || {
        echo "Evermore AI exited before capture could be inspected" >&2
        exit 1
    }
    grim "$TEST_HOME/clipboard-capture.png"
    if [[ -n "${EVERMORE_PHASE8_CLIPBOARD_SCREENSHOT:-}" ]]; then
        cp -- "$TEST_HOME/clipboard-capture.png" "$EVERMORE_PHASE8_CLIPBOARD_SCREENSHOT"
    fi
    wl-copy --clear >/dev/null 2>&1 || true
    echo "EVERMORE_CLIPBOARD_HOTKEY_LAUNCHED_OK"
fi
