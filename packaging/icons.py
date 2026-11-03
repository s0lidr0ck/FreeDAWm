#!/usr/bin/env python3
"""Regenerate every app icon from the owner's logo, assets/app-icon/freedawm-logo-source.png.

Needs Python 3 with Pillow (`pip install pillow`). The outputs are committed, so builds and
packaging never need it. Run from anywhere:

    python packaging/icons.py

Writes, in assets/app-icon/:
- freedawm-1024.png         the bird on a transparent 1024 px square (6 % margin);
- freedawm-macos-512.png    runtime window/Dock icon on macOS, on Apple's grid (an 824/1024 body);
- freedawm.icns             the macOS bundle icon, on the same grid;
- freedawm.ico              the Windows icon: 16, 24, 32, 48, 64, 128 and 256 px;
- hicolor/<n>x<n>/apps/io.github.s0lidr0ck.FreeDAWm.png   the Linux/FreeBSD icon theme, 16–512 px
  (named after the app id, which did not change in the rename).

and, for the app's own UI (embedded in the binary, crates/ui-egui/assets/brand/):
- freedawm-bird-192.png     the bird for the About box (96 pt at 2x), on a transparent square;
- freedawm-wordmark-720.png the wordmark (freedawm-wordmark-source.png) cropped to its letters,
  720 px wide (the About box's 360 pt at 2x; the menu bar draws it smaller, mipmapped).

At 48 px and below (taskbar, title bar, file lists) the thin gradient strokes wash out on light
backgrounds, so those sizes put the bird on a dark rounded tile (TILE_COLOR, corners TILE_RADIUS
of the side, the bird TILE_BIRD of the tile). 64 px and up, and the macOS icons (on Apple's
grid), stay transparent.
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
DIR = ROOT / "assets" / "app-icon"
SOURCE = DIR / "freedawm-logo-source.png"
WORDMARK = DIR / "freedawm-wordmark-source.png"
UI_DIR = ROOT / "crates" / "ui-egui" / "assets" / "brand"
APP_ID = "io.github.s0lidr0ck.FreeDAWm"
# The bird reaches the source's edges: keep it this far from every edge of the square.
MARGIN = 0.06
# Apple's grid: the icon body is 824 of 1024 px, with a transparent margin around it.
MAC_BODY = 824 / 1024
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
HICOLOR_SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
ICNS_SIZES = [16, 32, 64, 128, 256, 512, 1024]
# Small sizes: the bird on a dark rounded tile.
TILE_MAX = 48
TILE_COLOR = (0x16, 0x16, 0x1A, 255)
TILE_RADIUS = 0.22
TILE_BIRD = 0.84
SUPERSAMPLE = 8


def square(logo: Image.Image, size: int, body: float) -> Image.Image:
    """The logo, cropped to its visible pixels and centred on a transparent `size` square, its
    longer side `body * (1 - 2 * MARGIN)` of the square."""
    bird = logo.crop(logo.getchannel("A").getbbox())
    inner = size * body * (1 - 2 * MARGIN)
    scale = inner / max(bird.size)
    w, h = max(1, round(bird.width * scale)), max(1, round(bird.height * scale))
    bird = bird.resize((w, h), Image.LANCZOS)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.alpha_composite(bird, ((size - w) // 2, (size - h) // 2))
    return out


def tiled(logo: Image.Image, size: int) -> Image.Image:
    """The bird at TILE_BIRD of a dark rounded tile filling the `size` square. The tile is drawn
    SUPERSAMPLE times larger and scaled down, for smooth corners."""
    big = size * SUPERSAMPLE
    tile = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    ImageDraw.Draw(tile).rounded_rectangle((0, 0, big - 1, big - 1), radius=round(big * TILE_RADIUS), fill=TILE_COLOR)
    tile = tile.resize((size, size), Image.LANCZOS)
    bird = logo.crop(logo.getchannel("A").getbbox())
    scale = size * TILE_BIRD / max(bird.size)
    w, h = max(1, round(bird.width * scale)), max(1, round(bird.height * scale))
    tile.alpha_composite(bird.resize((w, h), Image.LANCZOS), ((size - w) // 2, (size - h) // 2))
    return tile


def main() -> None:
    logo = Image.open(SOURCE).convert("RGBA")
    # Small sizes are made from the full-resolution source each time (one resampling step).
    flat = lambda n: tiled(logo, n) if n <= TILE_MAX else square(logo, n, 1.0)  # noqa: E731
    mac = lambda n: square(logo, n, MAC_BODY)  # noqa: E731

    flat(1024).save(DIR / "freedawm-1024.png", optimize=True)
    mac(512).save(DIR / "freedawm-macos-512.png", optimize=True)

    for n in HICOLOR_SIZES:
        d = DIR / "hicolor" / f"{n}x{n}" / "apps"
        d.mkdir(parents=True, exist_ok=True)
        flat(n).save(d / f"{APP_ID}.png", optimize=True)

    icons = [flat(n) for n in ICO_SIZES]
    icons[-1].save(DIR / "freedawm.ico", format="ICO", sizes=[(n, n) for n in ICO_SIZES], append_images=icons[:-1])

    UI_DIR.mkdir(parents=True, exist_ok=True)
    square(logo, 192, 1.0 / (1 - 2 * MARGIN)).save(UI_DIR / "freedawm-bird-192.png", optimize=True)
    mark = Image.open(WORDMARK).convert("RGBA")
    mark = mark.crop(mark.getchannel("A").getbbox())
    w = 720
    mark.resize((w, round(mark.height * w / mark.width)), Image.LANCZOS).save(UI_DIR / "freedawm-wordmark-720.png", optimize=True)

    big = mac(1024)
    big.save(DIR / "freedawm.icns", format="ICNS", append_images=[mac(n) for n in ICNS_SIZES[:-1]])
    print(f"icons written to {DIR}")


if __name__ == "__main__":
    main()
