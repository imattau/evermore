# Evermore build VM

A headless Arch Linux VM for building the Evermore components (signing daemon,
settings daemon, egui panels, `npack`, etc.) in a clean, disposable environment
separate from the host.

- **Distro:** Arch Linux (official cloud image), matching the target deployment
  distro in [`../PROJECT_IDEA.md`](../PROJECT_IDEA.md).
- **Virtualization:** plain `qemu-system-x86_64` run directly, no libvirt. This
  needs no root/sudo — just membership in the `kvm` group (for `/dev/kvm`
  acceleration; it'll fall back to slow TCG emulation if that's missing) — and
  nothing more than the `qemu-img`/`qemu-system-x86_64`/`genisoimage` CLI tools.
  This also means it works in restricted/sandboxed shells where a libvirt
  daemon can't apply its own privilege-dropping (libvirt session mode was
  tried first and hit exactly that wall).
- **Networking:** QEMU's built-in user-mode networking (`-netdev user`), with
  guest SSH forwarded to `localhost:2222` on the host via `hostfwd`.
- **Disk:** a qcow2 overlay (`vm/.local/images/builder-disk.qcow2`) backed by
  the pristine downloaded base image, so the base image is never modified and
  the VM can be reset by deleting just the overlay (`destroy-vm.sh --wipe`).
- **Process management:** QEMU runs with `-daemonize`, a pidfile, and a Unix
  monitor socket (needs `socat` for graceful shutdown) — no systemd/libvirt
  daemon involved.
- **Display:** the VM has no virtual GPU and no QEMU-level display output —
  see "The display stack" below for why, and `vm/screenshot.sh` for how to
  actually see what's on screen.

## The display stack

The target is a Wayland desktop (Sway), so the VM needs to actually run one,
not just build headless daemons. Getting there took a few wrong turns worth
recording so they're not repeated:

- **`virtio-gpu-gl-pci` (GPU passthrough with 3D/virgl) doesn't work on this
  host.** Its QEMU module fails to load (`undefined symbol: qemu_egl_display`)
  — a packaging bug in this host's Ubuntu QEMU split, not something fixable
  from inside a VM script.
- **Plain `virtio-gpu-pci` (2D only) doesn't work either.** wlroots' DRM
  backend requires a renderer that supports DMA-BUF import for KMS scanout;
  neither the GL path (no 3D here) nor the pixman software renderer satisfy
  that against a 2D-only virtio-gpu device.
- **What works: Sway's `headless` backend + the `pixman` software renderer**,
  with no virtual GPU device in the QEMU command line at all
  (`WLR_BACKENDS=headless WLR_RENDERER=pixman`, set in `builder`'s
  `.bash_profile`, which also autologins on tty1 and execs `sway`). This is
  the same combination wlroots-based compositors use for their own headless
  CI testing, so it's well-trodden ground, not a hack specific to this setup.
- **`wayvnc` (the obvious way to get a live view) didn't pan out** — it
  accepted connections and reported the right resolution, but only ever
  served a flat grey frame regardless of what was actually on screen. Isolated
  the bug to wayvnc itself by comparing against `grim` (same underlying
  wlr-screencopy protocol, run locally inside the guest over SSH), which
  captured the real composited output perfectly on the first try. Since an
  on-demand screenshot is more useful than a live feed for scripted/AI-driven
  verification anyway, the live-VNC path was dropped rather than debugged
  further — `vm/screenshot.sh` does the same job (SSH in, run `grim`, scp the
  PNG back) with no extra moving parts.

## Directory layout

```
vm/
  README.md          <- this file (committed)
  create-vm.sh        <- (re)download image, build cloud-init seed, boot the VM (committed)
  connect.sh          <- SSH into the running VM (committed)
  sync-repo.sh         <- rsync the working tree into the VM, optionally run a command (committed)
  phase7-offline-inference.sh <- run a local model with egress blocked (committed)
  phase8-semantic-search.sh <- validate selected-file semantic search with egress blocked (committed)
  phase8-approval-deep-link.sh <- verify SwayNC approval notification through signed owner decision
  phase8-ai-proposal.sh <- verify real-model proposal through signed owner approval
  phase8-desktop-integration.sh <- run Send to AI and output-focus checks in disposable Sway
  phase8-launcher-input.sh <- regression check for the current launcher and deterministic Fuzzel pickers
  phase10-launcher-waybar.sh <- test Wofi app grid and Waybar in temporary input-enabled Sway
  phase8-send-to-ai.sh      <- validate file-manager AI handoff and .no-ai refusal in Sway
  phase8-ai-output-focus.sh <- verify the AI drawer closes when focused Sway output changes
  phase8-ai-window-focus.sh <- verify the AI drawer survives same-output window focus
  screenshot.sh         <- grab a PNG of the VM's Sway session via grim+SSH (committed)
  destroy-vm.sh           <- gracefully stop the VM, optionally wipe the disk (committed)
  .gitignore           <- ignores everything under .local/
  .local/               <- NOT committed: generated keys, disk images, cloud-init seed, runtime state
    ssh/id_ed25519(.pub) <- dedicated keypair for this VM only
    images/               <- base cloud image + the writable overlay disk
    seed/                  <- cloud-init user-data/meta-data/seed.iso
    run/                     <- qemu.pid, monitor.sock, console.log (serial console output)
```

## Phase 8 launcher keyboard check

The normal VM desktop uses a headless-only Sway backend, which has no input
devices. The Phase 8 regression check starts a separate temporary Sway session with
`headless,libinput` backends so wlroots attaches the VM's AT keyboard. It uses
the installed `wtype` helper to verify Super+Space, printable application
filtering, Enter launch, Escape dismissal, and dismissal after clicking outside
the launcher on the same output. The temporary XDG config and
compositor are removed when the check exits; the regular VM session is left
running.

```bash
vm/sync-repo.sh "bash vm/phase8-launcher-input.sh"
```

Set `EVERMORE_PHASE8_SCREENSHOT=/tmp/phase8-filtered.png` in the VM command to
preserve the filtered launcher screenshot for inspection. If the panel has been
built in `target/debug`, the same check also types a no-match file query,
verifies the semantic-result picker displays its row, and seeds the Wayland
clipboard to verify the Super+Shift+V binding launches the AI drawer. Set
`EVERMORE_PHASE8_SEMANTIC_SCREENSHOT=/tmp/phase8-semantic.png` to preserve the
semantic picker screenshot. Set
`EVERMORE_PHASE8_CLIPBOARD_SCREENSHOT=/tmp/phase8-clipboard.png` to keep a
screenshot showing the captured prompt text.

To exercise the warm-only AI suggestion alongside semantic file results, run
`EVERMORE_PHASE8_AI_FALLBACK_TEST=1 bash vm/phase8-launcher-input.sh` inside
the VM. This starts a fake inference socket that reports one already-loaded
chat model, checks the separate suggestion row copies its response, and
asserts the launcher sends `warm_only=true`. It tests launcher wiring and
selection behavior without downloading or starting a model.
Run `EVERMORE_PHASE8_AI_FALLBACK_CANCEL_TEST=1 bash
vm/phase8-launcher-input.sh` to hold the fake model after its first token,
press Escape in the result picker, and verify the launcher closes the inference
socket. Both modes use a local fake socket and require no model download.

Set `EVERMORE_PHASE8_WAYBAR_SCREENSHOT=/tmp/phase8-waybar.png` to verify that
Sway started Waybar and capture its panel before the launcher opens. This is
useful in the headless VM because the regular desktop session may not be
running when checking the packaged configuration.

## Phase 10 launcher and Waybar check

Wofi is the Super+Space desktop-app launcher. This check uses a separate Sway
session with libinput so it can send the hotkey, confirm the empty-query grid,
type the first search characters, launch Foot, and verify Escape and
outside-click dismissal. It also starts Waybar using the packaged configuration.
The temporary session and XDG config are removed on exit; the normal VM desktop
is left alone.

```bash
vm/sync-repo.sh "bash vm/phase10-launcher-waybar.sh"
```

Set `EVERMORE_PHASE10_GRID_SCREENSHOT=/tmp/phase10-grid.png` or
`EVERMORE_PHASE10_WAYBAR_SCREENSHOT=/tmp/phase10-waybar.png` in the VM command
to preserve screenshots for visual inspection.

Nothing under `.local/` is committed — it's host-specific generated state
(private key, multi-gigabyte disk images, a seed ISO, runtime sockets). Anyone
(human or AI coder) with this repo can regenerate it all from scratch by
running `create-vm.sh`.

## For other AI coders / fresh sessions: how to get in

1. If the VM isn't running yet, create/start it:
   ```bash
   vm/create-vm.sh
   ```
   This downloads the official Arch Linux cloud image (~550MB, once), builds a
  cloud-init seed that creates a `builder` user with passwordless sudo and
  installs `base-devel`, `git`, `rustup` (default toolchain set to stable),
  `rsync`, `llama-cpp`, `bubblewrap`, `iproute2`, the desktop
  components (`waybar`, `wofi`, `fuzzel`, `swaync`, `wl-clipboard`, `wtype`), and the
   GUI build dependencies egui/eframe need to compile
   (`wayland`, `wayland-protocols`, `libxkbcommon`, `mesa`, `fontconfig`) —
   boots QEMU directly, and polls until SSH answers. First boot takes roughly
   1-2 minutes while cloud-init provisions the user and runs package installs
   — the script waits for that automatically before returning. If it's already
   running, it just prints the existing pid and returns immediately.

2. Connect:
   ```bash
   vm/connect.sh
   # or run a one-off command:
   vm/connect.sh "cargo --version"
   ```
   This runs
   `ssh -i vm/.local/ssh/id_ed25519 -p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null builder@127.0.0.1`
   — the dedicated key lives at `vm/.local/ssh/id_ed25519` (gitignored,
   generated by `create-vm.sh` if missing).

3. Get the working tree in and build it there (the real target is Arch, not
   whatever the host runs — the VM is the source of truth for "does this
   actually build on the target system"):
   ```bash
   vm/sync-repo.sh                                    # just sync to ~/evermore
   vm/sync-repo.sh "cargo build --workspace"           # sync + build
   ```
   `target/` inside the VM is left alone across syncs (only the source tree
   is rsynced), so repeated syncs get incremental rebuilds.

4. The VM runs Sway automatically (headless + software rendering — see "The
   display stack" above), so GUI apps can actually run there too, not just
   build. To see what's on screen:
   ```bash
   vm/screenshot.sh                  # saves vm/.local/run/screenshot.png
   ```
   Launch an app against the VM's own Wayland session over SSH, e.g.:
   ```bash
   vm/connect.sh "XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-1 \
     nohup ~/evermore/target/debug/evermore-panel >/tmp/panel.log 2>&1 & disown"
   vm/screenshot.sh
   ```

5. You're now `builder` with passwordless sudo inside a disposable Arch VM.
   Clone/build Evermore components there, test `systemd --user` daemons, etc.
   without touching the host.

## Phase 7 offline model check

Install the Arch inference package in the VM once:

```bash
sudo pacman -S --noconfirm llama-cpp
```

After importing signed model packages so the model store contains
`manifest.json` plus the GGUFs under `~/.local/share/evermore/models`, run:

```bash
vm/sync-repo.sh "bash vm/phase7-offline-inference.sh"
```

The script copies the model store to a temporary directory and runs
`evermore-inferd` (which supervises `llama-server`) inside a Bubblewrap network
namespace with only loopback enabled. It checks the store scan, a real embedding
response, and then confirms an external TCP connection is unreachable. To also
test streamed chat against a locally imported model, set
`EVERMORE_P7_CHAT_MODEL=evermore/p7.model.tinyllama:1.0.0`. The VM needs
`bubblewrap`, `iproute2`, `llama-cpp`, and passwordless sudo; the host/VM
firewall and running desktop network are not changed. Set
`EVERMORE_P7_MODEL_DIR`, `EVERMORE_P7_MODEL`, `EVERMORE_P7_INFERD_BIN`, or
`EVERMORE_LLAMA_SERVER_BIN` in the VM to select a different model store, default
model, daemon binary, or `llama-server`.
With a chat model selected, the script also reports cold and warm first-token
latency and warm total-response time against the Phase 8 optional-launcher
budgets (300 ms first token and 3 s completion). A cold miss with a warm hit
means the launcher must wait until the model is already warm or suppress the
AI suggestion.

## Phase 8 semantic search check

After building `evermore-inferd` and `evermore-indexd` and importing the signed
BGE embedding package, run:

```bash
vm/sync-repo.sh "cargo build -p evermore-inferd -p evermore-indexd"
vm/sync-repo.sh "bash vm/phase8-semantic-search.sh"
```

This starts both daemons in a temporary Bubblewrap network namespace, indexes
one temporary allowlisted note, checks semantic retrieval and secret redaction,
measures warm search latency, and confirms a `.no-ai` marker hides the scope.
The model store and desktop configuration are copied or isolated; the host/VM
network settings and user service files are not changed. The embedding model
can be selected with `EVERMORE_P8_EMBED_MODEL` and the model store with
`EVERMORE_P8_MODEL_DIR`.

## Phase 8 Send to AI desktop action

With the headless Sway session running, build the panel and index daemon and
run:

```bash
vm/sync-repo.sh "cargo build -p evermore-panel -p evermore-indexd"
vm/sync-repo.sh "bash vm/phase8-send-to-ai.sh"
```

The check validates both desktop entries, launches the AI drawer with
temporary selected and `.no-ai` files, verifies the selected file is stored as
an unsent one-turn session grant, and confirms the excluded file is not added
to index scopes. It uses isolated XDG config/data/state directories and cleans
up its temporary services and files on exit.

Run the same integration checks without a live desktop using
`vm/sync-repo.sh "bash vm/phase8-desktop-integration.sh"`. The harness creates
a disposable headless Sway session, runs the Send to AI and focused-output
checks, confirms the AI drawer remains open when another window on the same
output receives focus, then removes its temporary session and files. The
focused-output script also verifies that a drawer opened on the second output
belongs to that output before confirming it closes when focus moves away.
Set `EVERMORE_PHASE8_VERIFY_INTERNAL_DRAG=1` to also start a fresh AI session,
drag the indexed file into its composer, and verify the saved one-turn grant.

`phase8-approval-deep-link.sh` validates the approval toast's Review action
through the explicit signed owner decision. `phase8-ai-proposal.sh` runs the
local Qwen model and checks the AI proposal through submission, owner review,
and the resulting settings change. Both create an explicit shared session
keyring for their disposable test identities and remove their test state when
finished.
Set `EVERMORE_PHASE8_INBOX_TEST=1` when running
`phase8-approval-deep-link.sh` to add an ordinary D-Bus notification, open the
SwayNC control center, and verify the local event and pending approval appear
once in the same inbox.

Run `vm/sync-repo.sh "cargo build -p evermore-panel && bash vm/phase8-ai-output-focus.sh"`
with the Sway session running to create a temporary headless output, open the
AI drawer on the focused output, switch output focus, and confirm the drawer
closes. The script removes its extra output and returns focus to the original.

## GPU training

This VM has no GPU passthrough and is not meant to train the adapter. Train on
a machine with a GPU using `packaging/training/` (optional Evermore training
profile) or the host toolchain, then copy the resulting GGUF here for
validation and packaging. Passthrough would require moving the host display to
its integrated GPU and binding the discrete GPU to `vfio-pci`; that is a
host-specific setup and is not configured by these scripts. The packaged
adapter is portable, so the VM does not need the GPU that trained it.

6. When done:
   ```bash
   vm/destroy-vm.sh         # gracefully powers off the VM, keeps the disk overlay
   vm/destroy-vm.sh --wipe  # also deletes the overlay disk (full reset to base image)
   ```

## Troubleshooting

- If `create-vm.sh` reports SSH never came up, check
  `vm/.local/run/console.log` for the guest's boot/cloud-init output.
- If the VM seems to be running but `connect.sh` refuses the connection,
  cloud-init is probably still mid-provisioning (package installs can take a
  couple of minutes on a slow link) — check the console log, it'll end with
  `Cloud-init v. ... finished at ...` once ready.
- Needs the `kvm` group (`groups` should list it) for hardware acceleration;
  without `/dev/kvm` access QEMU still works but is much slower (software
  emulation).
- Only port 2222→22 is forwarded to the host. If a later phase needs to reach
  another guest port (e.g. a debug port for an egui app), add another
  `hostfwd=tcp::<host>-:<guest>` segment to the `-netdev user,...` line in
  `create-vm.sh` (comma-separated, e.g.
  `user,id=net0,hostfwd=tcp::2222-:22,hostfwd=tcp::9229-:9229`).
- Resources are hardcoded in `create-vm.sh` (4 vCPU / 8GB RAM / 40GB disk
  overlay) — edit the script if you need more/less.
- **If a daemon you backgrounded with `nohup ... &` suddenly can't read its
  own kernel keyring entries** (`evermore-signer` returning `KeyRevoked` for
  an identity that worked moments ago), it's not a code bug: the Linux
  *session* keyring is scoped per PAM session, and Arch's default PAM config
  revokes a session's keyring when that session's last connection closes.
  `connect.sh`/`sync-repo.sh`/`screenshot.sh` all share one multiplexed SSH
  connection (`ControlMaster`/`ControlPersist`, socket at
  `vm/.local/run/ssh-control.sock`) specifically so a daemon backgrounded via
  one of these calls survives that call returning — but if you open your own
  raw `ssh` to the VM instead of using these scripts, or kill and restart the
  control socket between launching a daemon and using it, you can still hit
  this. `destroy-vm.sh` tears the control connection down along with the VM.
