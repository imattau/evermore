#!/usr/bin/env bash
# Stops and undefines the Evermore build VM. Pass --wipe to also delete the
# writable overlay disk (full reset back to the pristine base cloud image).
set -euo pipefail
VM_NAME="evermore-arch-builder"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

virsh -c qemu:///session destroy "$VM_NAME" >/dev/null 2>&1 || true
virsh -c qemu:///session undefine "$VM_NAME" >/dev/null 2>&1 || true

echo "==> domain stopped and undefined"

if [[ "${1:-}" == "--wipe" ]]; then
  rm -f "$SCRIPT_DIR/.local/images/builder-disk.qcow2"
  echo "==> overlay disk wiped (base image kept, re-run create-vm.sh to rebuild)"
fi
