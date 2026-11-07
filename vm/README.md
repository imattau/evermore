# Evermore build VM

A headless Arch Linux VM for building the Evermore components (signing daemon,
settings daemon, egui panels, `npack`, etc.) in a clean, disposable environment
separate from the host.

- **Distro:** Arch Linux (official cloud image), matching the target deployment
  distro in [`../PROJECT_IDEA.md`](../PROJECT_IDEA.md).
- **Virtualization:** libvirt **session** mode (`qemu:///session`) — no root/sudo
  needed on the host, so it works for any user (including other AI coding
  sessions) without elevated privileges.
- **Networking:** QEMU user-mode networking via the `passt` backend, with the
  guest's SSH port forwarded to `localhost:2222` on the host. No bridge, no NAT
  config, no system libvirtd required.
- **Disk:** a qcow2 overlay (`vm/.local/images/builder-disk.qcow2`) backed by the
  pristine downloaded base image, so the base image is never modified and the VM
  can be reset by deleting just the overlay.

## Directory layout

```
vm/
  README.md          <- this file (committed)
  create-vm.sh        <- (re)download image, build cloud-init seed, define+start the VM (committed)
  connect.sh          <- SSH into the running VM (committed)
  destroy-vm.sh        <- stop + undefine the VM, optionally wipe the disk (committed)
  .gitignore           <- ignores everything under .local/
  .local/               <- NOT committed: generated keys, disk images, cloud-init seed, domain XML
    ssh/id_ed25519(.pub) <- dedicated keypair for this VM only
    images/               <- base cloud image + the writable overlay disk
    seed/                  <- cloud-init user-data/meta-data/seed.iso
    domain.xml              <- the libvirt domain definition actually in use
```

Nothing under `.local/` is committed — it's host-specific generated state
(private key, multi-gigabyte disk images, a seed ISO). Anyone (human or AI
coder) with this repo can regenerate it all from scratch by running
`create-vm.sh`.

## For other AI coders / fresh sessions: how to get in

1. If the VM isn't running yet, create it:
   ```bash
   vm/create-vm.sh
   ```
   This downloads the official Arch Linux cloud image (~550MB, once), builds a
   cloud-init seed that creates a `builder` user with passwordless sudo and
   installs `base-devel`, `git`, `rustup`, defines the libvirt domain, and
   starts it. First boot takes 1-2 minutes while cloud-init provisions the
   user and runs `pacman` package installs.

2. Connect:
   ```bash
   vm/connect.sh
   ```
   This just runs
   `ssh -i vm/.local/ssh/id_ed25519 -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null builder@127.0.0.1`
   — the dedicated key lives at `vm/.local/ssh/id_ed25519` (gitignored, generated
   by `create-vm.sh` if missing).

3. You're now root-via-sudo inside a disposable Arch VM. Clone/build Evermore
   components there, test `systemd --user` daemons, etc. without touching the
   host.

4. When done:
   ```bash
   vm/destroy-vm.sh        # stops + undefines the domain, keeps the disk overlay
   vm/destroy-vm.sh --wipe  # also deletes the overlay disk (full reset to base image)
   ```

## Manual equivalents (if the scripts don't fit your situation)

```bash
# status
virsh -c qemu:///session list --all

# console (if SSH isn't up yet)
virsh -c qemu:///session console evermore-arch-builder

# start/stop an already-defined domain
virsh -c qemu:///session start evermore-arch-builder
virsh -c qemu:///session shutdown evermore-arch-builder
```

## Notes / gotchas

- The `passt` network backend requires `libvirt >= 9.0` and the `passt` package
  installed on the host (checked by `create-vm.sh`).
- Session-mode libvirt auto-spawns a per-user `libvirtd`/`virtqemud` on first
  `virsh` call and exits it again after a short idle timeout — this is normal
  and nothing to worry about.
- Resources are hardcoded in `create-vm.sh` (4 vCPU / 8GB RAM / 40GB disk
  overlay) — edit the script if you need more/less.
- The VM only exposes port 2222→22 to the host. If a future phase needs to
  reach a daemon's dev port (e.g. an egui app forwarding a debug port), add
  another `<range>` under `<portForward>` in the generated `domain.xml`, or
  extend `create-vm.sh`'s template.
