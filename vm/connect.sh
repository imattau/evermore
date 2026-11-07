#!/usr/bin/env bash
# SSH into the running Evermore build VM.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KEY="$SCRIPT_DIR/.local/ssh/id_ed25519"

if [[ ! -f "$KEY" ]]; then
  echo "no SSH key found at $KEY -- run vm/create-vm.sh first" >&2
  exit 1
fi

exec ssh -i "$KEY" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  -o LogLevel=ERROR \
  builder@127.0.0.1 "$@"
