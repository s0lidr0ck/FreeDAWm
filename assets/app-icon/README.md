# FreeDAWm app icon and wordmark

**The owner's FreeDAWm bird logo:** a bird in flight drawn in sweeping strokes, a blue → violet →
pink → orange gradient, with waveform bars in its body, on a transparent background. It replaces
the placeholder nightingale the app had as SoundCraft.

**The wordmark** spells "FreeDAWm": "Free" and "m" in white, "DAW" in the same gradient. The app
shows it in the About box and as the menu bar's first item; in the light theme its white letters
are tinted to the theme's text colour when the app loads it (`crates/ui-egui/src/brand_art.rs`).

## Provenance and licence

Both images were **supplied by the owner** for this private fork (see `ATTRIBUTION.md`). No
licence is stated; they are the owner's own work.

## Files

| File | What it is |
|---|---|
| `freedawm-logo-source.png` | The logo as supplied: 1536 × 1536 RGBA; the bird reaches the canvas edges |
| `freedawm-wordmark-source.png` | The wordmark as supplied: 1536 × 300 RGBA |
| `freedawm-1024.png` | The bird on a transparent 1024 px square, 6 % margin on every side |
| `freedawm-macos-512.png` | Runtime window/Dock icon on macOS, on Apple's grid (an 824/1024 body with a transparent margin) |
| `freedawm.icns` | macOS bundle icon (16–1024 px, Apple's grid) |
| `freedawm.ico` | Windows icon (16, 24, 32, 48, 64, 128, 256 px), embedded in `freedawm.exe` and used by the MSI |
| `hicolor/<size>x<size>/apps/io.github.s0lidr0ck.FreeDAWm.png` | Linux/FreeBSD icon theme, 16–512 px (named after the app id, which the rename kept) |

At 48 px and below (the `.ico` and `hicolor` small sizes) the bird sits on a dark rounded tile
(`#16161a`, corners 22 % of the side, the bird 84 % of the tile), so its colours read on a light
taskbar. 64 px and up, and the macOS icons, stay transparent.

There is no `scalable/` SVG any more: the logo is a raster image.

Regenerate everything from the source with `packaging/icons.sh` (or `python packaging/icons.py`;
needs Pillow). At 16 px the waveform bars inside the bird blur together; the wings and the
outline still read.
