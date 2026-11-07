#!/usr/bin/env bash
# Creates (or starts) the Evermore Arch Linux build VM using plain QEMU/KVM
# directly (no libvirt) -- no root needed, just /dev/kvm access (the `kvm`
# group). Guest SSH is forwarded to localhost:2222 via QEMU user-mode
# networking.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCAL_DIR="$SCRIPT_DIR/.local"
IMAGES_DIR="$LOCAL_DIR/images"
SEED_DIR="$LOCAL_DIR/seed"
SSH_DIR="$LOCAL_DIR/ssh"
RUN_DIR="$LOCAL_DIR/run"

BASE_IMAGE_URL="https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2"
BASE_IMAGE="$IMAGES_DIR/Arch-Linux-x86_64-cloudimg.qcow2"
OVERLAY_DISK="$IMAGES_DIR/builder-disk.qcow2"
DISK_SIZE="40G"
VM_MEMORY="8G"
VM_VCPUS="4"
SSH_PORT="2222"

PIDFILE="$RUN_DIR/qemu.pid"
MONITOR_SOCK="$RUN_DIR/monitor.sock"
CONSOLE_LOG="$RUN_DIR/console.log"

for bin in qemu-system-x86_64 qemu-img genisoimage curl ssh-keygen; do
  command -v "$bin" >/dev/null || { echo "missing required tool: $bin" >&2; exit 1; }
done

if [[ ! -e /dev/kvm ]]; then
  echo "warning: /dev/kvm not present -- VM will run (slowly) under TCG emulation" >&2
fi

mkdir -p "$IMAGES_DIR" "$SEED_DIR" "$SSH_DIR" "$RUN_DIR"

if [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
  echo "==> VM already running (pid $(cat "$PIDFILE"))"
  echo "==> $SCRIPT_DIR/connect.sh"
  exit 0
fi
rm -f "$PIDFILE" "$MONITOR_SOCK"

if [[ ! -f "$SSH_DIR/id_ed25519" ]]; then
  echo "==> generating dedicated SSH keypair for the VM"
  ssh-keygen -t ed25519 -f "$SSH_DIR/id_ed25519" -N "" -C "evermore-arch-builder" -q
fi
PUBKEY="$(cat "$SSH_DIR/id_ed25519.pub")"

if [[ ! -f "$BASE_IMAGE" ]]; then
  echo "==> downloading Arch Linux cloud image (~550MB)"
  curl -fSL -o "$BASE_IMAGE" "$BASE_IMAGE_URL"
fi

if [[ ! -f "$OVERLAY_DISK" ]]; then
  echo "==> creating ${DISK_SIZE} writable overlay disk"
  qemu-img create -f qcow2 -F qcow2 -b "$BASE_IMAGE" "$OVERLAY_DISK" "$DISK_SIZE"
fi

echo "==> writing cloud-init seed"
cat > "$SEED_DIR/meta-data" <<EOF
instance-id: evermore-arch-builder-01
local-hostname: evermore-arch-builder
EOF

cat > "$SEED_DIR/user-data" <<EOF
#cloud-config
hostname: evermore-arch-builder
fqdn: evermore-arch-builder.local
manage_etc_hosts: true

users:
  - name: builder
    groups: [wheel, seat, video, input]
    shell: /bin/bash
    sudo: ["ALL=(ALL) NOPASSWD:ALL"]
    ssh_authorized_keys:
      - ${PUBKEY}

ssh_pwauth: false
disable_root: true

growpart:
  mode: auto
  devices: ["/"]

package_update: true
package_upgrade: false

packages:
  - base-devel
  - git
  - openssh
  - pkgconf
  - rustup
  - rsync
  - wayland
  - wayland-protocols
  - libxkbcommon
  - mesa
  - fontconfig
  - sway
  - foot
  - seatd
  - grim
  - swaybg

write_files:
  - path: /etc/systemd/system/getty@tty1.service.d/autologin.conf
    permissions: '0644'
    content: |
      [Service]
      ExecStart=
      ExecStart=-/sbin/agetty --autologin builder --noclear %I \$TERM

runcmd:
  - systemctl enable --now sshd
  - systemctl enable --now seatd
  # written here, not via write_files: /home/builder doesn't exist yet
  # when write_files runs (it's created later, by the users module)
  - |
    cat > /home/builder/.bash_profile <<'PROFILE'
    if [ -z "\$WAYLAND_DISPLAY" ] && [ "\$(tty)" = "/dev/tty1" ]; then
      export WLR_BACKENDS=headless
      export WLR_RENDERER=pixman
      exec sway > /tmp/sway.log 2>&1
    fi
    PROFILE
  - chown builder:builder /home/builder/.bash_profile
  - systemctl daemon-reload
  - systemctl restart getty@tty1.service
  - [su, builder, -c, "rustup default stable"]
  - touch /var/lib/cloud/evermore-provisioned
EOF

genisoimage -output "$SEED_DIR/seed.iso" -volid cidata -joliet -rock \
  "$SEED_DIR/user-data" "$SEED_DIR/meta-data" >/dev/null

echo "==> starting qemu"
qemu-system-x86_64 \
  -name evermore-arch-builder \
  -machine q35,accel=kvm:tcg \
  -cpu host \
  -m "$VM_MEMORY" \
  -smp "$VM_VCPUS" \
  -drive file="$OVERLAY_DISK",if=virtio,format=qcow2 \
  -drive file="$SEED_DIR/seed.iso",if=virtio,media=cdrom \
  -netdev "user,id=net0,hostfwd=tcp::${SSH_PORT}-:22" \
  -device virtio-net-pci,netdev=net0 \
  -display none \
  -serial file:"$CONSOLE_LOG" \
  -monitor unix:"$MONITOR_SOCK",server,nowait \
  -pidfile "$PIDFILE" \
  -daemonize

echo "==> VM started (pid $(cat "$PIDFILE")). Waiting for SSH on 127.0.0.1:${SSH_PORT}..."
for i in $(seq 1 90); do
  if (exec 3<>"/dev/tcp/127.0.0.1/${SSH_PORT}") 2>/dev/null; then
    exec 3>&- 3<&-
    echo "==> SSH is up. Run: $SCRIPT_DIR/connect.sh"
    echo "==> Display: $SCRIPT_DIR/screenshot.sh (grabs a PNG of the Sway session)"
    exit 0
  fi
  sleep 2
done

echo "==> SSH didn't come up within 3 minutes. Check $CONSOLE_LOG for boot output." >&2
exit 1
