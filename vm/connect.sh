#!/usr/bin/env bash
# SSH into the running Evermore build VM.
#
# Uses SSH connection multiplexing (ControlMaster/ControlPersist) so
# repeated connect.sh/sync-repo.sh/screenshot.sh calls share ONE underlying
# SSH connection (and therefore one PAM session) instead of each opening a
# fresh login. This matters beyond just speed: the Linux kernel's *session*
# keyring is scoped per PAM session, and some PAM configs (Arch's default
# included) revoke a session's keyring when that session's last connection
# closes -- so `evermore-signer serve` backgrounded under one `connect.sh`
# call would have its keys silently revoked the moment that particular SSH
# connection ended, even though the process kept running. One shared,
# persistent control connection avoids that entirely. See vm/README.md.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KEY="$SCRIPT_DIR/.local/ssh/id_ed25519"

if [[ ! -f "$KEY" ]]; then
  echo "no SSH key found at $KEY -- run vm/create-vm.sh first" >&2
  exit 1
fi

mkdir -p "$SCRIPT_DIR/.local/run"

exec ssh -i "$KEY" \
  -p 2222 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  -o LogLevel=ERROR \
  -o ControlMaster=auto \
  -o ControlPersist=10m \
  -o ControlPath="$SCRIPT_DIR/.local/run/ssh-control.sock" \
  builder@127.0.0.1 "$@"
