# evermore-greeter / greetd packaging

`config.toml` here is the `greetd` config that replaces the stock
`agreety`/`tuigreet` frontend with `evermore-greeter` (an eframe/egui
app showing each identity's `kind:0`-derived name/avatar, run inside
`cage`, a kiosk Wayland compositor) -- see PROJECT_IDEA.md's Phase 5.5.

## Dependencies

- `greetd` (official Arch package)
- `cage` (official Arch package) -- the kiosk compositor evermore-greeter runs inside
- `evermore-greeter` itself on `$PATH` (built from `apps/evermore-greeter`)
- `evermore-enroll` and `evermore-signer` in `/usr/bin` for first-user setup and login key loading

On real hardware with a GPU, that's everything: `cage` picks up the
real GPU driver automatically. **On a headless VM with no GPU (e.g.
this project's own `vm/` test environment) one more package is
needed**: `vulkan-swrast` (lavapipe, a software Vulkan implementation).
Without it, Mesa's EGL falls back to `zink` (OpenGL-over-Vulkan) and
fails outright with `VK_ERROR_INCOMPATIBLE_DRIVER` since no Vulkan ICD
is registered at all -- confirmed by reading the loader's own debug
output (`VK_LOADER_DEBUG=all`), not guessed. `vulkan-swrast` registers
`/usr/share/vulkan/icd.d/lvp_icd.json`, after which rendering works via
`llvmpipe`, verified by actually screenshotting a running eframe app
under `cage` headless with `grim` (see `vm/README.md`'s display-stack
notes for why `grim`/wlr-screencopy is this project's standard way to
see what a headless Wayland session is showing).

## Install

```sh
sudo cp config.toml /etc/greetd/config.toml
sudo systemctl restart greetd
```

Back up `/etc/greetd/config.toml` first if you want the stock config
back -- this overwrites it, it doesn't merge.

## First-user enrollment

On an empty install, install `evermore-first-user.service` from
`packaging/systemd/` and enable it before enabling greetd:

```sh
sudo install -m 0644 packaging/systemd/evermore-first-user.service /usr/lib/systemd/system/
sudo systemctl enable evermore-first-user.service
sudo systemctl enable greetd.service
```

The required dependency makes greetd wait for enrollment on first boot. The
service runs `evermore-enroll first-user` as root on tty1. It supports a new
local identity or importing an `nsec`, creates a LUKS-backed homed account,
stores its key in the encrypted home with mode 0600, and writes the display
name cache used by the greeter. For newly generated identities it displays
the recovery `nsec` once; record it somewhere safe. The user's signer service
loads the key into the login session keyring before it accepts requests.

If enrollment fails, switch to another text terminal, inspect
`journalctl -u evermore-first-user.service`, resolve the reported issue, and
restart the unit. The greeter reports when no account was created. This is an
early local-passphrase path; NIP-46 pairing is not supported by this setup.
