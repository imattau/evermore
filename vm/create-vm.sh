#!/usr/bin/env bash
# Creates (or recreates) the Evermore Arch Linux build VM using libvirt
# session mode (qemu:///session, no root needed) with QEMU user-mode
# networking (passt backend) forwarding guest SSH to localhost:2222.
set -euo pipefail

VM_NAME="evermore-arch-builder"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCAL_DIR="$SCRIPT_DIR/.local"
IMAGES_DIR="$LOCAL_DIR/images"
SEED_DIR="$LOCAL_DIR/seed"
SSH_DIR="$LOCAL_DIR/ssh"

BASE_IMAGE_URL="https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2"
BASE_IMAGE="$IMAGES_DIR/Arch-Linux-x86_64-cloudimg.qcow2"
OVERLAY_DISK="$IMAGES_DIR/builder-disk.qcow2"
DISK_SIZE="40G"
VM_MEMORY_GIB="8"
VM_VCPUS="4"
SSH_PORT="2222"

for bin in virsh qemu-img genisoimage curl ssh-keygen; do
  command -v "$bin" >/dev/null || { echo "missing required tool: $bin" >&2; exit 1; }
done

if ! virsh -c qemu:///session connect --readonly >/dev/null 2>&1; then
  echo "warning: could not probe qemu:///session; continuing anyway" >&2
fi

mkdir -p "$IMAGES_DIR" "$SEED_DIR" "$SSH_DIR"

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
instance-id: ${VM_NAME}-01
local-hostname: ${VM_NAME}
EOF

cat > "$SEED_DIR/user-data" <<EOF
#cloud-config
hostname: ${VM_NAME}
fqdn: ${VM_NAME}.local
manage_etc_hosts: true

users:
  - name: builder
    groups: [wheel]
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

runcmd:
  - systemctl enable --now sshd
  - [su, builder, -c, "rustup default stable"]
  - touch /var/lib/cloud/evermore-provisioned
EOF

genisoimage -output "$SEED_DIR/seed.iso" -volid cidata -joliet -rock \
  "$SEED_DIR/user-data" "$SEED_DIR/meta-data" >/dev/null

echo "==> writing libvirt domain definition"
cat > "$LOCAL_DIR/domain.xml" <<EOF
<domain type='kvm'>
  <name>${VM_NAME}</name>
  <memory unit='GiB'>${VM_MEMORY_GIB}</memory>
  <currentMemory unit='GiB'>${VM_MEMORY_GIB}</currentMemory>
  <vcpu>${VM_VCPUS}</vcpu>
  <os>
    <type arch='x86_64' machine='q35'>hvm</type>
    <boot dev='hd'/>
  </os>
  <features>
    <acpi/>
    <apic/>
  </features>
  <cpu mode='host-passthrough'/>
  <clock offset='utc'/>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>restart</on_reboot>
  <on_crash>destroy</on_crash>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type='file' device='disk'>
      <driver name='qemu' type='qcow2'/>
      <source file='${OVERLAY_DISK}'/>
      <target dev='vda' bus='virtio'/>
    </disk>
    <disk type='file' device='cdrom'>
      <driver name='qemu' type='raw'/>
      <source file='${SEED_DIR}/seed.iso'/>
      <target dev='sda' bus='sata'/>
      <readonly/>
    </disk>
    <interface type='user'>
      <backend type='passt'/>
      <model type='virtio'/>
      <portForward proto='tcp'>
        <range start='${SSH_PORT}' to='22'/>
      </portForward>
    </interface>
    <console type='pty'/>
    <video>
      <model type='none'/>
    </video>
  </devices>
</domain>
EOF

if virsh -c qemu:///session dominfo "$VM_NAME" >/dev/null 2>&1; then
  echo "==> domain already defined, redefining"
  virsh -c qemu:///session destroy "$VM_NAME" >/dev/null 2>&1 || true
  virsh -c qemu:///session undefine "$VM_NAME" >/dev/null 2>&1 || true
fi

virsh -c qemu:///session define "$LOCAL_DIR/domain.xml"
virsh -c qemu:///session start "$VM_NAME"

echo
echo "==> VM started. First boot takes 1-2 minutes to provision via cloud-init."
echo "==> Once up: $SCRIPT_DIR/connect.sh"
