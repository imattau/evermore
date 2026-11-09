# QEMU desktop image

`build-image.sh` produces a clean Arch Linux x86_64 qcow2 for the first
bootable Evermore desktop milestone. It builds the workspace in release mode,
builds the embedded relay supervisor, boots a temporary Arch cloud image,
installs `packages.x86_64`, stages Evermore's binaries, system setup, and
offline model store, then flattens the disk to
`dist/evermore-arch-x86_64.qcow2`. The default store must include BGE small
embeddings and Qwen2.5-1.5B-Instruct with the Evermore Qwen LoRA. TinyLlama is
rejected. The Qwen model is selected by default for new users.

This workflow does not touch `vm/.local` or the developer VM. The output has
no human account, no SSH access, and no cloud-init provisioning state. On its
first graphical boot it starts Evermore's greeter; the guided first-user
screen can create a local identity or pair a NIP-46 signer when networking is
available. `qrencode` is included for pairing QR codes. The host store defaults
to `.image-build/model-store` and can be overridden with
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
