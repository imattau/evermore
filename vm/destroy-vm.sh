#!/usr/bin/env bash
# Stops the Evermore build VM (plain QEMU process, no libvirt). Pass --wipe
# to also delete the writable overlay disk (full reset to the base image).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN_DIR="$SCRIPT_DIR/.local/run"
PIDFILE="$RUN_DIR/qemu.pid"
MONITOR_SOCK="$RUN_DIR/monitor.sock"

pid_alive() {
  [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE" 2>/dev/null)" 2>/dev/null
}

if pid_alive; then
  if command -v socat >/dev/null && [[ -S "$MONITOR_SOCK" ]]; then
    echo "==> sending graceful shutdown via QEMU monitor"
    echo "system_powerdown" | socat - "UNIX-CONNECT:$MONITOR_SOCK" >/dev/null 2>&1 || true
    for i in $(seq 1 30); do
      pid_alive || break
      sleep 1
    done
  fi
  if pid_alive; then
    echo "==> still running, sending quit via monitor"
    echo "quit" | socat - "UNIX-CONNECT:$MONITOR_SOCK" >/dev/null 2>&1 || true
    sleep 1
  fi
else
  echo "==> VM not running"
fi

rm -f "$PIDFILE" "$MONITOR_SOCK"
echo "==> VM stopped"

if [[ "${1:-}" == "--wipe" ]]; then
  rm -f "$SCRIPT_DIR/.local/images/builder-disk.qcow2"
  echo "==> overlay disk wiped (base image kept, re-run create-vm.sh to rebuild)"
fi
