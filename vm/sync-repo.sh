#!/usr/bin/env bash
# Syncs the working tree into the VM at ~/evermore (excludes build
# artifacts, .git, and the VM's own generated state) and optionally runs a
# command there afterward.
#
#   vm/sync-repo.sh                      # just sync
#   vm/sync-repo.sh "cargo build --workspace"
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
KEY="$SCRIPT_DIR/.local/ssh/id_ed25519"

SSH_OPTS=(-i "$KEY" -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR)

if [[ ! -f "$KEY" ]]; then
  echo "no SSH key at $KEY -- run vm/create-vm.sh first" >&2
  exit 1
fi

command -v rsync >/dev/null || { echo "rsync not found on host" >&2; exit 1; }

rsync -az --delete -e "ssh ${SSH_OPTS[*]}" \
  --exclude target --exclude vm/.local --exclude .git \
  "$REPO_ROOT/" builder@127.0.0.1:~/evermore/

if [[ $# -gt 0 ]]; then
  ssh "${SSH_OPTS[@]}" builder@127.0.0.1 "cd ~/evermore && $*"
fi
