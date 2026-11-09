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
- `qrencode` (official Arch package) for the first-user NIP-46 pairing QR
- `sudo` for the greeter's restricted first-user helper commands

### Optional phone unlock pairing

Build and register the Evermore PKCS#11 module before first boot if you want
the setup wizard to offer phone approval:

```sh
cargo build --release -p evermore-pkcs11
sudo install -Dm0755 target/release/libevermore_pkcs11.so /usr/lib/pkcs11/evermore-pkcs11.so
sudo install -Dm0644 packaging/pkcs11/evermore-pkcs11.module /usr/share/p11-kit/modules/evermore-pkcs11.module
sudo p11-kit list-modules
```

The module provides homed's PKCS#11 unlock factor. The greeter pairs the
device's NIP-46 client identity with a phone, validates the connect secret and
`get_public_key` response, and stores the remote signer and relay in the
root-only device state. The homed record receives both the PKCS#11 factor and
the regular passphrase factor. If the relay is offline or the user skips
pairing, setup continues with the passphrase factor alone.

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

Install the greeter's narrowly scoped enrollment rule, then enable greetd:

```sh
sudo install -m 0644 packaging/systemd/evermore-signer.service /usr/lib/systemd/user/
sudo install -m 0440 packaging/greetd/evermore-first-user.sudoers /etc/sudoers.d/evermore-first-user
sudo systemctl --global enable evermore-signer.service
sudo systemctl enable greetd.service
```

With no homed accounts, the graphical greeter opens the guided first-user
flow. It supports a new local identity or importing an `nsec`, creates a
LUKS-backed homed account, stores its key in the encrypted home with mode 0600,
and writes the display name cache used by the greeter. For a new identity it
shows the recovery `nsec` once and requires acknowledgement before proceeding.
When the PKCS#11 module is installed and a relay is reachable, the wizard can
pair a phone with a QR code and enroll phone approval for home unlock. The
home passphrase remains available as an offline fallback. The user's signer
service loads the identity key into the login session keyring after login.
The system-wide signer unit is enabled for every user so the account does not
need a Rust toolchain in its home.

If enrollment fails, the greeter reports the helper error and allows retry.
Failures after homed has created the account require inspecting the homed
record and home before removing anything. The helper will not alter existing
accounts.
