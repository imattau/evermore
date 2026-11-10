#!/usr/bin/env python3
"""Generate the Evermore wallpaper/lock-screen background set.

Procedural rather than AI-rendered, on purpose: the Evermore Graphite
design note's palette is a set of exact hex tokens (see
PROJECT_IDEA.md's "Design tokens" section, and apps/evermore-panel's
build_visuals/apps/evermore-greeter's build_visuals, which both encode
the same tuples), and a raster model can only approximate those, not
reproduce them. This script is the single source of truth for the
wallpaper set the way theme.toml is meant to be for the rest of the
palette -- rerunning it regenerates every size/variant identically.

Produces three dark variants (graphite, dusk, frost) at 2560x1440 plus
an ultrawide (3440x1440) crop each, per PROJECT_IDEA.md's "Iconography"
section ("Wallpapers: three dark variants ... applied with
`swaybg -m fill`").
"""
import numpy as np
from PIL import Image

# Exact tokens from PROJECT_IDEA.md's "Design tokens" table / the panel
# and greeter's build_visuals -- not approximated.
INK_950 = np.array([14, 16, 20], dtype=np.float64)  # canvas, lock screen
INK_900 = np.array([20, 23, 29], dtype=np.float64)  # chrome
INK_850 = np.array([26, 30, 37], dtype=np.float64)  # cards
ACCENT = np.array([110, 168, 254], dtype=np.float64)  # #6EA8FE
ACCENT_STRONG = np.array([139, 183, 255], dtype=np.float64)  # #8BB7FF
WARNING = np.array([229, 181, 103], dtype=np.float64)  # #E5B567

SIZES = {
    "": (2560, 1440),
    "-ultrawide": (3440, 1440),
}


def smoothstep(t):
    t = np.clip(t, 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def base_gradient(width, height):
    """Soft diagonal sweep from ink-950 (top-left) to ink-850
    (bottom-right), kept subtle (never reaching full ink-850) so large
    flat regions stay calm enough for UI text on top."""
    xs = np.linspace(0.0, 1.0, width)
    ys = np.linspace(0.0, 1.0, height)
    xx, yy = np.meshgrid(xs, ys)
    u = smoothstep((xx * 0.6 + yy * 0.4))
    # Cap the sweep at 65% of the way to ink-850, not the full token gap.
    t = (u * 0.65)[..., None]
    return INK_950[None, None, :] + t * (INK_850 - INK_950)[None, None, :]


def radial_glow(width, height, center, radius, color, intensity):
    """Additive soft radial glow anchored near one corner/edge, never
    reaching the frame center at meaningful strength -- the design
    brief this was generated from called for a "calm, uncluttered
    center" so greeter/first-boot text and buttons stay legible.

    `intensity` is a 0-1 fraction of `color`'s own brightness, not a
    raw multiplier -- a first pass that multiplied by ~20 saturated
    the whole glow to solid white well before the falloff curve had a
    chance to matter, producing a visible sun disc instead of an
    ambient tint."""
    cx, cy = center
    xs = np.linspace(0.0, 1.0, width)
    ys = np.linspace(0.0, 1.0, height)
    xx, yy = np.meshgrid(xs, ys)
    # Aspect-correct distance so the glow isn't stretched on ultrawide.
    aspect = width / height
    dx = (xx - cx) * aspect
    dy = yy - cy
    dist = np.sqrt(dx * dx + dy * dy)
    # Inverse-square-ish falloff (not a linear/smoothstep ramp) so the
    # glow reads as a gradual ambient bleed with no visible disc edge.
    falloff = 1.0 / (1.0 + (dist / radius) ** 2)
    falloff = np.clip(falloff, 0.0, 1.0) * intensity
    return falloff[..., None] * color[None, None, :]


def vignette(width, height, strength=0.10):
    """Barely-there corner darkening for depth; multiplicative so it
    can't blow out the glow highlights."""
    xs = np.linspace(-1.0, 1.0, width)
    ys = np.linspace(-1.0, 1.0, height)
    xx, yy = np.meshgrid(xs, ys)
    dist = np.sqrt(xx * xx + yy * yy) / np.sqrt(2.0)
    return 1.0 - strength * smoothstep(dist)


def grain(width, height, sigma=3.2, seed=0):
    """Soft film-grain texture, not per-pixel digital noise: generated
    at a tenth resolution and upscaled, which reads as organic grain
    *and* keeps the PNG compressible -- full-resolution per-pixel
    noise defeated PNG's predictor/LZ77 compression almost entirely,
    turning ~200KB gradients into 4-5MB files."""
    rng = np.random.default_rng(seed)
    small = rng.normal(
        0.0, sigma, size=(max(1, height // 10), max(1, width // 10))
    )
    noise = Image.fromarray(small.astype(np.float32), mode="F").resize(
        (width, height), resample=Image.BILINEAR
    )
    return np.asarray(noise)[..., None]


def render(width, height, variant, seed):
    img = base_gradient(width, height)

    if variant == "dusk":
        img = img + radial_glow(
            width, height, center=(0.14, 1.08), radius=0.42,
            color=WARNING, intensity=0.55,
        )
    elif variant == "frost":
        img = img + radial_glow(
            width, height, center=(0.88, -0.08), radius=0.40,
            color=ACCENT_STRONG, intensity=0.5,
        )
    # "graphite" is the neutral base gradient alone -- no accent glow.

    img = img * vignette(width, height)[..., None]
    img = img + grain(width, height, seed=seed)
    img = np.clip(img, 0, 255).astype(np.uint8)
    return Image.fromarray(img, mode="RGB")


def main():
    import pathlib

    out_dir = pathlib.Path(__file__).resolve().parent
    for variant, seed in [("graphite", 1), ("dusk", 2), ("frost", 3)]:
        for suffix, (width, height) in SIZES.items():
            image = render(width, height, variant, seed)
            path = out_dir / f"{variant}{suffix}.png"
            image.save(path, optimize=True, compress_level=9)
            print(f"wrote {path} ({width}x{height})")


if __name__ == "__main__":
    main()
