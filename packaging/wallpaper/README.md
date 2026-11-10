# Evermore wallpaper set

Three dark variants -- `graphite`, `dusk`, `frost` -- at 2560x1440 plus an
ultrawide (3440x1440) crop each, per PROJECT_IDEA.md's "Iconography"
section. `generate.py` is the source of truth; the PNGs here are its
checked-in output so nothing at runtime depends on regenerating them.

- `graphite.png` / `graphite-ultrawide.png` -- the neutral base: a soft
  diagonal sweep between `ink-950` and `ink-850`, no accent color. Used as
  `evermore-greeter`'s lock-screen and first-boot background (see
  `apps/evermore-greeter/src/main.rs`'s `load_background_texture`).
- `dusk.png` / `dusk-ultrawide.png` -- the same base with a faint warm
  `warning` (`#E5B567`) glow low in one corner.
- `frost.png` / `frost-ultrawide.png` -- the same base with a faint cool
  `accent-strong` (`#8BB7FF`) glow in the opposite corner.

## Why procedural, not AI-rendered

The palette is a set of exact hex tokens (see PROJECT_IDEA.md's "Design
tokens" table, and `apps/evermore-panel`'s and `apps/evermore-greeter`'s
`build_visuals`, which both encode the same tuples) -- a raster model can
only approximate those, not reproduce them pixel-for-pixel, and can't be
rerun deterministically when a token changes. `generate.py` reads the same
tuples the two egui apps use and composites a soft diagonal gradient, an
optional corner glow, a light vignette, and a low-resolution-upscaled grain
pass (full-resolution per-pixel noise defeated PNG compression,
turning ~200KB gradients into multi-megabyte files).

## Regenerating

```bash
python3 packaging/wallpaper/generate.py
```

Requires `numpy` and `Pillow`. Writes all six PNGs into this directory in
place.

## Desktop usage

Apply with `swaybg -m fill` from Sway config, e.g.:

```
output * bg packaging/wallpaper/graphite.png fill
```

Use the `-ultrawide` crop on outputs wider than 21:9; `swaybg` does not
crop for you, so the wrong aspect will distort on `fill`.
