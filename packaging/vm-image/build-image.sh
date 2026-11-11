#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE_DIR="${EVERMORE_IMAGE_WORKDIR:-$ROOT/.image-build}"
OUTPUT_DIR="${EVERMORE_IMAGE_OUTPUT_DIR:-$ROOT/dist}"
MODEL_STORE="${EVERMORE_IMAGE_MODEL_STORE:-$IMAGE_DIR/vm-model-store}"
BASE_URL="${EVERMORE_ARCH_CLOUD_IMAGE_URL:-https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2}"
BASE_SOURCE="${EVERMORE_ARCH_CLOUD_IMAGE:-$ROOT/vm/.local/images/Arch-Linux-x86_64-cloudimg.qcow2}"
MEMORY="${EVERMORE_IMAGE_MEMORY:-6G}"; VCPUS="${EVERMORE_IMAGE_VCPUS:-4}"
DISK_SIZE="${EVERMORE_IMAGE_DISK_SIZE:-32G}"; PORT="${EVERMORE_IMAGE_SSH_PORT:-2239}"
NPACK_UPSTREAM_REV="900b836cc743d6036fe284750b8e688468e01685" # v0.9.4
VM_PID=""
for bin in cargo go git python3 qemu-system-x86_64 qemu-img genisoimage curl ssh ssh-keygen rsync; do
  command -v "$bin" >/dev/null || { echo "missing required host tool: $bin" >&2; exit 1; }
done
mkdir -p "$IMAGE_DIR" "$OUTPUT_DIR"
IMAGE_DIR="$(cd "$IMAGE_DIR" && pwd)"; OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"
STAGE="$IMAGE_DIR/stage"; NPACK_SRC="$IMAGE_DIR/npack-src"; BASE="$IMAGE_DIR/Arch-Linux-x86_64-cloudimg.qcow2"
DISK="$IMAGE_DIR/evermore-build.qcow2"; SEED="$IMAGE_DIR/seed"; KEY="$IMAGE_DIR/id_ed25519"
OUT="$OUTPUT_DIR/evermore-arch-x86_64.qcow2"
cleanup() { if [[ -n "$VM_PID" ]] && kill -0 "$VM_PID" 2>/dev/null; then kill "$VM_PID" 2>/dev/null || true; wait "$VM_PID" 2>/dev/null || true; fi; }
trap cleanup EXIT
rm -rf "$STAGE"; mkdir -p "$STAGE/bin" "$STAGE/lib" "$STAGE/share" "$STAGE/systemd/user"
[[ -d "$MODEL_STORE" ]] || { echo "model store missing: $MODEL_STORE" >&2; exit 1; }
python3 - "$MODEL_STORE" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
required = {
    "p7.model.bge",
    "p8.model.qwen2.5-1.5b.mcp",
    "p8.model.qwen2.5-1.5b.panel",
}
seen = set()
manifests = {}
for manifest_path in root.glob("*/manifest.json"):
    manifest = json.loads(manifest_path.read_text())
    package_id = manifest.get("package_id")
    seen.add(package_id)
    manifests[package_id] = manifest
    artifacts = [manifest["base"]]
    if manifest.get("adapter"):
        artifacts.append(manifest["adapter"])
    for artifact in artifacts:
        path = manifest_path.parent / artifact["file"]
        if not path.is_file():
            raise SystemExit(f"missing model artifact: {path}")
        hasher = hashlib.sha256()
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                hasher.update(chunk)
        digest = hasher.hexdigest()
        if digest != artifact["sha256"]:
            raise SystemExit(f"SHA-256 mismatch: {path}")
if missing := required - seen:
    raise SystemExit(f"missing required offline models: {', '.join(sorted(missing))}")
mcp = manifests["p8.model.qwen2.5-1.5b.mcp"]
panel = manifests["p8.model.qwen2.5-1.5b.panel"]
expected_adapters = {
    "p8.model.qwen2.5-1.5b.mcp": "e73fd3251fea6a9a2c6e2949eed50ac9f744674ee274288e0c074b2431ba11e0",
    "p8.model.qwen2.5-1.5b.panel": "3be70918f009efbb1906bb8d592c035e32d6d168498969542eb5f562df2225a9",
}
for package_id, model in (("p8.model.qwen2.5-1.5b.mcp", mcp), ("p8.model.qwen2.5-1.5b.panel", panel)):
    if not model.get("adapter") or not model.get("training"):
        raise SystemExit(f"{package_id} must include its adapter and training provenance")
    if model["adapter"].get("sha256") != expected_adapters[package_id]:
        raise SystemExit(f"unexpected adapter selected for {package_id}")
if mcp["base"].get("sha256") != panel["base"].get("sha256"):
    raise SystemExit("MCP and panel adapters must use the same Qwen base model")
if any("tinyllama" in path.name.lower() for path in root.iterdir()):
    raise SystemExit("TinyLlama is not supported in the default image")
PY
echo '==> Building release binaries'
mkdir -p "$IMAGE_DIR/go-cache"
(cd "$ROOT" && cargo build --release --workspace)
if [[ ! -d "$NPACK_SRC/.git" ]] || [[ "$(git -C "$NPACK_SRC" rev-parse HEAD 2>/dev/null || true)" != "$NPACK_UPSTREAM_REV" ]]; then
  rm -rf "$NPACK_SRC"
  git clone --quiet --depth 1 --branch v0.9.4 https://github.com/imattau/npack.git "$NPACK_SRC"
fi
[[ "$(git -C "$NPACK_SRC" rev-parse HEAD)" == "$NPACK_UPSTREAM_REV" ]] || {
  echo "unexpected npack source revision; expected v0.9.4 at $NPACK_UPSTREAM_REV" >&2
  exit 1
}
cargo build --manifest-path "$NPACK_SRC/Cargo.toml" --locked --release -p npack -p npack-gui
for name in npack npack-gui; do
  source="$NPACK_SRC/target/release/$name"
  [[ -x "$source" ]] || { echo "npack release binary missing: $source" >&2; exit 1; }
  install -m 0755 "$source" "$STAGE/bin/$name"
done
[[ "$("$STAGE/bin/npack" --version)" == "npack 0.9.4" ]] || {
  echo 'unexpected npack CLI version in staged image' >&2
  exit 1
}
install -m 0644 "$NPACK_SRC/packaging/npackd.service" "$STAGE/systemd/user/"
install -m 0644 "$NPACK_SRC/packaging/desktop/io.npack.Gui.desktop" "$STAGE/share/"
install -m 0644 "$NPACK_SRC/packaging/desktop/io.npack.Gui.svg" "$STAGE/share/"
install -m 0644 "$NPACK_SRC/packaging/desktop/io.npack.Gui.metainfo.xml" "$STAGE/share/"
(cd "$ROOT/relay" && GOCACHE="$IMAGE_DIR/go-cache" CGO_ENABLED=0 go build -trimpath -ldflags='-s -w' -o "$IMAGE_DIR/evermore-relay-supervisor" .)
for name in evermore-greeter evermore-enroll evermore-signer evermore-panel evermore-contextd evermore-indexd evermore-inferd evermore-npack evermore-operationsd evermore-notifyd evermore-settingsd evermore-blossom-fs evermore-sway-ai-bridge evermore-agentd; do
  source="$ROOT/target/release/$name"
  [[ -x "$source" ]] || { echo "release binary missing: $source" >&2; exit 1; }
  install -m 0755 "$source" "$STAGE/bin/$name"
done
install -m 0755 "$IMAGE_DIR/evermore-relay-supervisor" "$STAGE/bin/evermore-relay-supervisor"
install -m 0755 "$ROOT/target/release/libevermore_pkcs11.so" "$STAGE/lib/evermore-pkcs11.so"
if [[ ! -f "$BASE" ]]; then
  if [[ -f "$BASE_SOURCE" ]]; then cp --reflink=auto "$BASE_SOURCE" "$BASE"
  else curl -fSL "$BASE_URL" -o "$BASE"
  fi
fi
rm -f "$DISK"; qemu-img create -f qcow2 -F qcow2 -b "$BASE" "$DISK" "$DISK_SIZE" >/dev/null
mkdir -p "$SEED"; [[ -f "$KEY" ]] || ssh-keygen -q -t ed25519 -N '' -f "$KEY" -C evermore-image-builder
cat > "$SEED/meta-data" <<'EOF'
instance-id: evermore-image-build
local-hostname: evermore
EOF
{
  PACKAGE_NAMES="$(sed -e '/^[[:space:]]*#/d' -e '/^[[:space:]]*$/d' \
    "$ROOT/packaging/vm-image/packages.x86_64" | tr '\n' ' ')"
  cat <<EOF
#cloud-config
hostname: evermore
manage_etc_hosts: true
users:
  - name: builder
    groups: [wheel]
    shell: /bin/bash
    sudo: ["ALL=(ALL) NOPASSWD:ALL"]
    ssh_authorized_keys:
      - $(cat "$KEY.pub")
ssh_pwauth: false
disable_root: true
package_update: false
runcmd:
  - systemctl enable --now sshd
  - |
    set -eu
    pacman -Syu --noconfirm
    pacman -S --noconfirm --needed $PACKAGE_NAMES
    for package in $PACKAGE_NAMES; do pacman -Q "\$package" >/dev/null; done
    touch /var/lib/cloud/evermore-image-ready
EOF
} > "$SEED/user-data"
genisoimage -quiet -output "$SEED/seed.iso" -volid cidata -joliet -rock "$SEED/user-data" "$SEED/meta-data"
echo "==> Starting clean Arch provisioning VM on SSH port $PORT"
if [[ -s "$IMAGE_DIR/qemu.pid" ]]; then
  old_pid="$(cat "$IMAGE_DIR/qemu.pid")"
  if kill -0 "$old_pid" 2>/dev/null; then
    echo "image-builder QEMU is already running with pid $old_pid; stop it before rebuilding" >&2
    exit 1
  fi
  rm -f "$IMAGE_DIR/qemu.pid"
fi
qemu-system-x86_64 -name evermore-image-build -machine q35,accel=kvm:tcg -cpu max -m "$MEMORY" -smp "$VCPUS" \
  -drive "file=$DISK,if=virtio,format=qcow2" -drive "file=$SEED/seed.iso,if=virtio,media=cdrom" \
  -netdev "user,id=net0,hostfwd=tcp::$PORT-:22" -device virtio-net-pci,netdev=net0 -display none \
  -serial file:"$IMAGE_DIR/console.log" -pidfile "$IMAGE_DIR/qemu.pid" \
  >"$IMAGE_DIR/qemu.stdout" 2>&1 &
VM_PID=$!
SSH=(ssh -i "$KEY" -p "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=3)
ready=0
for _ in $(seq 1 180); do
  if "${SSH[@]}" builder@127.0.0.1 'test -e /var/lib/cloud/evermore-image-ready' 2>/dev/null; then ready=1; break; fi
  sleep 2
done
if (( ! ready )); then echo "Arch provisioning did not finish; inspect $IMAGE_DIR/console.log" >&2; exit 1; fi
echo '==> Installing Evermore files and enabling graphical first boot'
"${SSH[@]}" builder@127.0.0.1 'mkdir -p /tmp/evermore/packaging'
RSYNC_RSH="ssh -i $KEY -p $PORT -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null" rsync -a --delete "$STAGE/" "builder@127.0.0.1:/tmp/evermore-stage/"
RSYNC_RSH="ssh -i $KEY -p $PORT -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null" rsync -aH --delete "$MODEL_STORE/" "builder@127.0.0.1:/tmp/evermore-model-store/"
RSYNC_RSH="ssh -i $KEY -p $PORT -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null" rsync -a "$ROOT/packaging/" "builder@127.0.0.1:/tmp/evermore/packaging/"
"${SSH[@]}" builder@127.0.0.1 'sudo bash /tmp/evermore/packaging/vm-image/install-image.sh /tmp/evermore-stage /tmp/evermore-model-store'
echo '==> Shutting down and flattening image'
# -f: this command runs over builder's own SSH session, so plain `userdel`
# always refuses with "user builder is currently used by process ..." --
# silently, since stderr is thrown away below. That left /etc/passwd's
# builder entry (and its wheel/sudo membership) in every past build of
# this image even though /home/builder itself was gone, because `rm -rf`
# isn't short-circuited by userdel's exit code. Verified live: dropping -f
# reproduces the leftover entry; -f removes it cleanly in the same run.
"${SSH[@]}" builder@127.0.0.1 'sudo sh -c "userdel -f -r builder 2>/dev/null || true; rm -rf /home/builder; systemctl poweroff"' || true
for _ in $(seq 1 60); do kill -0 "$VM_PID" 2>/dev/null || break; sleep 2; done
if kill -0 "$VM_PID" 2>/dev/null; then kill "$VM_PID"; wait "$VM_PID" 2>/dev/null || true; fi
VM_PID=""; rm -f "$IMAGE_DIR/qemu.pid"
qemu-img convert -p -O qcow2 "$DISK" "$OUT"
qemu-img check "$OUT"; sha256sum "$OUT" > "$OUT.sha256"
rm -f "$KEY" "$KEY.pub"
echo "==> Image ready: $OUT"
