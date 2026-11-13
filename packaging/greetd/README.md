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
- `pam_evermore_phone.so` in `/usr/lib/security` for the greeter's explicit phone-login route
- `qrencode` (official Arch package) for the first-user NIP-46 pairing QR
- `sudo` for the greeter's restricted first-user helper commands

### Optional phone unlock pairing

Build and register the Evermore PKCS#11 module before first boot if you want
the setup wizard to offer phone approval:

```sh
cargo build --release -p evermore-pkcs11
cargo build --release -p evermore-pam-phone
sudo install -Dm0755 target/release/libevermore_pkcs11.so /usr/lib/pkcs11/evermore-pkcs11.so
sudo install -Dm0755 target/release/libpam_evermore_phone.so /usr/lib/security/pam_evermore_phone.so
sudo install -Dm0644 packaging/pkcs11/evermore-pkcs11.module /usr/share/p11-kit/modules/evermore-pkcs11.module
sudo p11-kit list-modules
```

The module provides homed's PKCS#11 unlock factor. Each account gets a
separate NIP-46 client identity, relay, and remote signer pairing under
`/var/lib/evermore/users/<username>` with root-only permissions. The greeter
selects the account's pairing at unlock time, and the phone approval request
names the account. The homed record receives that account's PKCS#11 factor
and keeps its regular passphrase factor. Pairing is available during
first-user setup; the root enrollment helper accepts a username so the same
flow can be wired into multi-account provisioning. If the relay is offline
or the user skips pairing, setup continues with the passphrase factor alone.

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

At sign-in, accounts enrolled with the phone factor show **Use phone** beside
the passphrase action. Choosing it activates the home through the PKCS#11
provider and waits for the paired NIP-46 signer to approve. The passphrase
continues to work as the offline fallback.

If enrollment fails after `systemd-homed` creates the new account, the helper
attempts to remove that account and its greeter profile before allowing retry.
If cleanup itself fails, the error names the account and says to inspect it
before retrying. The helper never removes an account that existed before this
enrollment attempt. Sudden power loss during setup is not yet automatically
recovered; see [application readiness](../../docs/application-readiness.md).
