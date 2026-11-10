# QEMU desktop image

`build-image.sh` produces a clean Arch Linux x86_64 qcow2 for the first
bootable Evermore desktop milestone. It builds the workspace in release mode,
builds the embedded relay supervisor, boots a temporary Arch cloud image,
installs `packages.x86_64`, stages Evermore's binaries, system setup, and
offline model store, then flattens the disk to
`dist/evermore-arch-x86_64.qcow2`. The model store includes BGE small
embeddings and two Qwen2.5-1.5B-Instruct task adapters: MCP v9 epoch 208 and
panel v10 epoch 41. The MCP runtime selects the `.mcp` model; the first-user
desktop profile selects the `.panel` model. TinyLlama is rejected.

This is a usable engineering image with the best currently selected separate
adapters. The MCP adapter scored 410/418 tool selections and 396/418
schema-valid calls on the full development set; the panel adapter scored
151/154 operation selections and 136/154 schema-valid proposals. The locked
end-to-end release evaluation has not passed, so this image is for VM use and
continued evaluation rather than a certified release.

The image also builds and installs upstream `npack` v0.9.4 (pinned to commit
`900b836cc743d6036fe284750b8e688468e01685`), including `npack-gui`, its
launcher and its per-user daemon. The daemon starts with each user session;
new homes receive Evermore's default trusted-publisher and relay configuration.

This workflow does not touch `vm/.local` or the developer VM. The output has
no human account, no SSH access, and no cloud-init provisioning state. On its
first graphical boot it starts Evermore's greeter; the guided first-user
screen can create a local identity or pair a NIP-46 signer when networking is
available. `qrencode` is included for pairing QR codes. The host store defaults
to `.image-build/vm-model-store` and can be overridden with
`EVERMORE_IMAGE_MODEL_STORE`. Artifacts are SHA-256 checked before the build
starts and installed as read-only files under `/var/lib/evermore/models`.
The qcow2 was booted in a disposable QEMU snapshot: the serial log reached
the graphical greeter, and the screen showed **Begin setup** followed by the
guided account-name step. The image itself was not modified by that check.

## Build

On a Linux x86_64 host with QEMU, `qemu-img`, `genisoimage`, `curl`, `rsync`,
OpenSSH, Rust/Cargo, and Go installed:

```sh
packaging/vm-image/build-image.sh
```

The first build downloads Arch's current cloud image and Arch packages. It
uses KVM when available and falls back to QEMU TCG. The output has a 32 GiB
virtual capacity. `EVERMORE_IMAGE_OUTPUT_DIR`, `EVERMORE_IMAGE_WORKDIR`,
`EVERMORE_IMAGE_MEMORY`, `EVERMORE_IMAGE_VCPUS`, `EVERMORE_IMAGE_DISK_SIZE`,
and `EVERMORE_IMAGE_SSH_PORT` can override the defaults. Arch package versions
are not pinned yet, so the procedure is repeatable but not bit-for-bit
reproducible.

Boot it with:

```sh
qemu-system-x86_64 -enable-kvm -m 4G -smp 4 \
  -drive file=dist/evermore-arch-x86_64.qcow2,if=virtio,format=qcow2 \
  -device virtio-vga -display gtk -netdev user,id=n0 -device virtio-net-pci,netdev=n0
```

This is the online, VM-oriented milestone. The offline signed package
repository, archiso installer, device-root trust bootstrap, UKI setup, and
unattended install acceptance harness remain Phase 9 work; this qcow2 is not
the offline installer.
