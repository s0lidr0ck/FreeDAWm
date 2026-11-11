# The FreeDAWm guide

A user guide made of cheat sheets: one idea per page, a picture with lettered callouts first and
words second. It is one HTML file styled for US Letter pages and exported to PDF.

- `guide.html`: every page, in order (each `<section class="page">` is one sheet). Page numbers,
  "see page N" references (`<span data-page="id">`) and the contents list are filled in by the
  script at the end of the file, so pages can be added or moved without renumbering.
- `guide.css`: the page furniture (title strip, cards, "The Goal" notes, callout pins, key caps).
- `images/`: screenshots rendered from the app by `shots.py`. `fonts/`: Fraunces and Inter.
- `menus.py`: writes the Menu Map pages from `crates/ui-egui/src/menubar.txt` and the command
  registry's shortcuts. Run it after the menu bar changes.
- `FreeDAWm-Guide.pdf`: the last export.

## Build

```sh
node docs/guide/build.cjs                 # writes docs/guide/FreeDAWm-Guide.pdf
node docs/guide/build.cjs out.pdf --png DIR   # also one PNG per page
```

It needs Node with Playwright's Chromium. It exits non-zero when a sheet has outgrown its page.
Open `guide.html` in a browser to read the pages without building.

## Screenshots

Render them from the current app so they never go stale (needs Pillow for the crops):

```sh
python3 docs/guide/shots.py            # every shot
python3 docs/guide/shots.py armed io   # only these
```

Each shot is a `ui_shot` script in `shots/`; `shots.py` lists its window size, flags and crops.
Most session shots use a sample project that is not in the repository (its audio is not ours to
redistribute). Point `FREEDAWM_GUIDE_PROJECT` at a folder holding `Sample.fdawm` and its
`Audio Files/`; `shots/_sample.jsonl` opens it and builds the FireStation mix, the key, the markers
and the MIDI part on top. Shots that do not need it still render without it.
The demo song still loads the hidden stock effects, so the mixer shots first run
`shots/_firestation.jsonl`, which swaps them for FireStation modules.

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
