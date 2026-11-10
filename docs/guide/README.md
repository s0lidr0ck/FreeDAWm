# The FreeDAWm guide

A user guide made of cheat sheets: one idea per page, a picture with lettered callouts first and
words second. It is one HTML file styled for US Letter pages and exported to PDF.

- `guide.html`: every page, in order (each `<section class="page">` is one sheet).
- `guide.css`: the page furniture (title strip, cards, "The Goal" notes, callout pins, key caps).
- `images/`: screenshots rendered from the app. `fonts/`: Fraunces (titles) and Inter (text).
- `FreeDAWm-Guide.pdf`: the last export.

## Build

```sh
node docs/guide/build.cjs                 # writes docs/guide/FreeDAWm-Guide.pdf
node docs/guide/build.cjs out.pdf --png DIR   # also one PNG per page
```

It needs Node with Playwright's Chromium. It exits non-zero when a sheet has outgrown its page.
Open `guide.html` in a browser to read the pages without building.

## Screenshots

Render them from the current app, from the repository root, so they never go stale:

```sh
cargo run -p soundcraft-ui-egui --example ui_shot -- /tmp/last.png docs/guide/shots.jsonl --size 1440x900 --system-theme dark
```

The callouts are positioned in percentages of the picture (`.zone` boxes in `guide.html`), so after
a layout change in the app, check that each box still sits on its part of the window.

## Writing a sheet

- One idea per page. If a page needs a second idea, it is two pages.
- Start with "The Goal": one or two sentences on what the reader will be able to do.
- Prefer a labelled picture, a 2×2 or a table to paragraphs. Keep notes to two lines.
- Write `Cmd` for the main modifier (the note on page 2 tells Windows, Linux and FreeBSD readers to
  press Ctrl) and take shortcuts from the command registry, not from memory.
- Screenshots come from `ui_shot` and the demo session only. Diagrams are drawn for this guide.
  Nothing is copied from another product's documentation.
