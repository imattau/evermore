# evermore-greeter / greetd packaging

`config.toml` here is the `greetd` config that replaces the stock
`agreety`/`tuigreet` frontend with `evermore-greeter` (an eframe/egui
app showing each identity's `kind:0`-derived name/avatar, run inside
`cage`, a kiosk Wayland compositor) -- see PROJECT_IDEA.md's Phase 5.5.

## Dependencies

- `greetd` (official Arch package)
- `cage` (official Arch package) -- the kiosk compositor evermore-greeter runs inside
- `evermore-greeter` itself on `$PATH` (built from `apps/evermore-greeter`)

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
