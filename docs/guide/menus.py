#!/usr/bin/env python3
"""Write the guide's Menu Map pages from the menu bar itself: `python3 docs/guide/menus.py`.

Reads crates/ui-egui/src/menubar.txt (the menu bar as shown) and replaces what is between the
`menus:start` and `menus:end` comments in guide.html. Run it after the menu bar changes.
"""
import html
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[2]
GUIDE = ROOT / "docs/guide/guide.html"

# What each menu is for, and how the menus share pages (a page of one menu uses two columns).
ABOUT = {
    "FreeDAWm": "About the app, facts about the session, and Quit.",
    "File": "Sessions: make, open, save, and bring things in or send them out.",
    "Edit": "Everything you do to the selection: clipboard, cuts, fades, trims and automation.",
    "View": "What the windows show: rulers, counters, clip details and mixer sections.",
    "Track": "Make, organise, freeze and route tracks.",
    "Clip": "Act on the selected clips: group, lock, loop, gain and timing.",
    "Event": "Time, tempo and MIDI operations.",
    "Process": "Offline processing: the effect is written into the selected audio.",
    "AI": "Tools that run on your computer. See Section 7.",
    "Options": "Switches for how recording, playback and editing behave.",
    "Setup": "Your audio device, inputs, shortcuts, preferences and AI models.",
    "Window": "Open the windows, editors and panels, and arrange them.",
}
PAGES = [["FreeDAWm", "File", "Setup"], ["Edit"], ["View", "Clip"], ["Track"], ["Event", "Options"], ["Process", "AI"], ["Window"]]
APP_MENU = [("", ["About FreeDAWm", "Session Info", "Session Audio Health", "Quit"])]


def shortcuts() -> dict:
    """Menu path -> shortcut, from the command registry (`freedawm-cli commands` has no keys, so
    the engine's and the UI's command tables are read directly)."""
    keys = {}
    pattern = re.compile(r'"[a-z_0-9.]+",\s*"([^"]+)",\s*(?:&?\[([^\]]*)\]|"([^"]*)"),\s*Some\("([^"]+)"\)')
    for path in [*sorted((ROOT / "crates/engine/src/cmd").glob("*.rs")), ROOT / "crates/ui-egui/src/menus.rs"]:
        for label, menu, ui_path, key in pattern.findall(path.read_text()):
            parts = re.findall(r'"([^"]+)"', menu) + [label] if menu else [p.strip() for p in ui_path.split(">")]
            if len(parts) > 1 and "," not in key and "-" not in key and "/" not in key[:-1]:
                keys[" > ".join(parts)] = key
    return keys


def parse() -> dict:
    """menu -> [(group heading, [(item, [submenu children] | None, catalog path)])], in file order."""
    menus: dict = {}
    for raw in (ROOT / "crates/ui-egui/src/menubar.txt").read_text().splitlines():
        line = raw.split("//")[0].strip()
        if not line:
            continue
        line = line.replace("[desktop]", "").strip()
        shown, _, target = line.partition(" = ")
        parts = [p.strip() for p in shown.split(" > ")]
        menu, rest = parts[0], parts[1:]
        groups = menus.setdefault(menu, [])
        if rest[0].startswith("## "):
            groups.append((rest[0][3:], []))
            continue
        if rest[-1] == "---":
            continue
        if not groups:
            groups.append(("", []))
        items = groups[-1][1]
        soon = target.strip().startswith("soon")
        label = rest[-1] + (" (coming soon)" if soon else "")
        path = target.strip() if " > " in target else shown.strip()
        if len(rest) == 1:
            items.append((label, None, path))
        else:
            sub = " › ".join(rest[:-1])
            for found in (g for g in groups for g in g[1] if g[0] == sub and g[1] is not None):
                found[1].append(label)
                break
            else:
                items.append((sub, [label], path))
    return menus


def item_html(label: str, children, path: str, keys: dict) -> str:
    key = keys.get(path)
    if children is None:
        kbd = "<b>" + " ".join(f"<kbd>{html.escape(k)}</kbd>" for k in key.split("+")) + "</b>" if key else ""
        return f"<li>{html.escape(label)}{kbd}</li>"
    shown = children if len(children) <= 12 else children[:11] + [f"and {len(children) - 11} more"]
    return f'<li class="sub">{html.escape(label)} <i>›</i> <span>{html.escape(", ".join(shown))}</span></li>'


def main() -> None:
    menus = {"FreeDAWm": [(g, [(i, None, "") for i in items]) for g, items in APP_MENU], **parse()}
    keys = shortcuts()
    out = []
    for page in PAGES:
        title = (page[0] + " Menu") if len(page) == 1 else ", ".join(page[-3:-1]) + " &amp; " + page[-1]
        out.append(f"<!-- Menu map: {', '.join(page)} -->")
        out.append(f'<section class="page" id="menu-{page[0].lower()}">')
        out.append('  <header class="strip">')
        out.append('    <div class="mark"><img src="../../assets/app-icon/freedawm-1024.png" alt=""><span>Free<span class="daw">DAW</span>m</span></div>')
        out.append(f"    <h1>{title}</h1>")
        out.append("  </header>")
        out.append(f'  <div class="menus{" one" if len(page) == 1 else ""}">')
        for name in page:
            out.append('    <div class="menu">')
            out.append(f"      <h2>{name}</h2>")
            out.append(f"      <p>{ABOUT[name]}</p>")
            out.append('      <div class="groups">')
            for heading, items in menus[name]:
                out.append('        <div class="group">')
                if heading:
                    out.append(f"          <h3>{html.escape(heading)}</h3>")
                out.append("          <ul>" + "".join(item_html(*i, keys) for i in items) + "</ul>")
                out.append("        </div>")
            out.append("      </div>")
            out.append("    </div>")
        out.append("  </div>")
        out.append("</section>")
        out.append("")
    text = GUIDE.read_text()
    start, end = "<!-- menus:start -->\n", "<!-- menus:end -->"
    a, b = text.index(start) + len(start), text.index(end)
    GUIDE.write_text(text[:a] + "\n".join(out) + text[b:])
    print(f"wrote {len(PAGES)} menu pages into docs/guide/guide.html")


if __name__ == "__main__":
    main()
