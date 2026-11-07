#!/usr/bin/env bash
# Grabs a PNG of whatever the VM's Sway session is currently showing and
# saves it to the host. Sway runs headless inside the guest (no real GPU --
# the host's QEMU build has broken virtio-gpu-gl modules, and software KMS
# scanout isn't supported by current wlroots either -- see vm/README.md).
# `grim` talks wlr-screencopy directly over the guest's own Wayland socket,
# so this just runs it over SSH and copies the result back.
#
#   vm/screenshot.sh                  # saves to vm/.local/run/screenshot.png
#   vm/screenshot.sh /tmp/out.png     # saves wherever you like
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KEY="$SCRIPT_DIR/.local/ssh/id_ed25519"
OUT="${1:-$SCRIPT_DIR/.local/run/screenshot.png}"

mkdir -p "$SCRIPT_DIR/.local/run"
# Shares one SSH connection with connect.sh/sync-repo.sh -- see the comment
# in connect.sh for why (PAM session / kernel keyring scoping).
CONTROL_OPTS=(-o ControlMaster=auto -o ControlPersist=10m -o ControlPath="$SCRIPT_DIR/.local/run/ssh-control.sock")
SSH_OPTS=(-i "$KEY" -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "${CONTROL_OPTS[@]}")
SCP_OPTS=(-i "$KEY" -P 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "${CONTROL_OPTS[@]}")

if [[ ! -f "$KEY" ]]; then
  echo "no SSH key at $KEY -- run vm/create-vm.sh first" >&2
  exit 1
fi

ssh "${SSH_OPTS[@]}" builder@127.0.0.1 \
  'XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1 grim /tmp/evermore-screenshot.png'

scp "${SCP_OPTS[@]}" builder@127.0.0.1:/tmp/evermore-screenshot.png "$OUT" >/dev/null

echo "==> saved $OUT"
