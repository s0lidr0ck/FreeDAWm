#!/usr/bin/env python3
"""Render the guide's screenshots from the current app: `python3 docs/guide/shots.py [name…]`.

Each shot is a ui_shot script in docs/guide/shots/ that writes docs/guide/images/<name>.png, then a
crop (left, top, right, bottom in window points; ui_shot renders two pixels per point). Needs
Pillow for the crops. Run from anywhere in the repository.
"""
import os
import pathlib
import subprocess
import sys

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[2]
# script: (images it writes, window size, extra ui_shot flags, crops)
# A crop is (output image, source image, box in points); a source with no crop is kept whole.
# "--firestation" is ours, not ui_shot's: the demo song still loads the hidden stock effects, so
# the script is run after shots/_firestation.jsonl, which swaps them for FireStation modules.
FS = "--firestation"
# "--sample" is ours too: the shot uses the sample project (a folder holding Sample.fdawm and its
# Audio Files, named by $FREEDAWM_GUIDE_PROJECT; its audio is not in the repository), opened and
# given a FireStation mix by shots/_sample.jsonl.
SAMPLE = "--sample"
SHOTS = {
    "windows": (["edit-window", "mix-window"], "1440x900", [SAMPLE], [("markers-ruler", "edit-window", (180, 114, 1188, 243))]),
    "engine": (["playback-engine"], "1440x900", [], [("playback-engine", "playback-engine", (16, 16, 476, 376))]),
    "io": (["io-setup"], "1440x900", [], [("io-setup", "io-setup", (16, 16, 577, 546))]),
    "new-tracks": (["new-tracks"], "1440x900", [], [("new-tracks", "new-tracks", (438, 317, 1002, 423))]),
    "midi": (
        ["midi-editor", "midi-notes"],
        "1440x900",
        [SAMPLE],
        [("midi-editor", "midi-editor", (8, 529, 1432, 848)), ("midi-notes", "midi-notes", (8, 529, 1432, 848))],
    ),
    "markers": (["memory-locations"], "1440x900", [SAMPLE], [("memory-locations", "memory-locations", (160, 120, 641, 288))]),
    "mix-menus": (
        ["add-effect", "send-menu"],
        "1440x900",
        [SAMPLE],
        [("add-effect", "add-effect", (481, 101, 1049, 691)), ("send-menu", "send-menu", (481, 374, 1080, 591))],
    ),
    "effect": (["effect-drawer"], "1440x900", [SAMPLE], [("effect-drawer", "effect-drawer", (176, 101, 1290, 720))]),
    "automation": (["automation-lane"], "1440x900", [SAMPLE], [("automation-lane", "automation-lane", (180, 423, 760, 510))]),
    "juke": (
        ["juke-kit", "juke-steps"],
        "1280x860",
        ["--blank"],
        [("juke-kit", "juke-kit", (289, 117, 1280, 817)), ("juke-steps", "juke-steps", (289, 117, 1280, 817))],
    ),
    "lift": (["lift"], "1280x860", ["--blank"], [("lift", "lift", (180, 0, 1280, 860))]),
    "browser": (
        ["browser-instruments", "browser-effects"],
        "1440x900",
        [],
        [("browser-instruments", "browser-instruments", (878, 108, 1200, 540)), ("browser-effects", "browser-effects", (878, 108, 1200, 842))],
    ),
    "vocal-editor": (["vocal-editor"], "1500x1100", [SAMPLE], [("vocal-editor", "vocal-editor", (175, 355, 1250, 1030))]),
    "vocal-studio": (["vocal-studio"], "1500x1000", [SAMPLE], [("vocal-studio", "vocal-studio", (177, 352, 1260, 931))]),
    "ai": (
        ["ai-separate", "ai-midi", "ai-song"],
        "1440x900",
        [SAMPLE],
        [("ai-separate", "ai-separate", (8, 22, 590, 322)), ("ai-midi", "ai-midi", (11, 10, 357, 480)), ("ai-song", "ai-song", (11, 10, 357, 577))],
    ),
    "models": (["ml-models"], "1300x820", [], [("ml-models", "ml-models", (9, 15, 1103, 727))]),
    "bounce": (["bounce-mix"], "1440x900", [], [("bounce-mix", "bounce-mix", (524, 238, 917, 502))]),
    "armed": (
        ["armed-track"],
        "1720x900",
        ["--blank"],
        [("record-transport", "armed-track", (1114, 33, 1664, 101)), ("armed-track", "armed-track", (176, 200, 447, 291))],
    ),
}


def main() -> int:
    names = sys.argv[1:] or list(SHOTS)
    subprocess.run(["cargo", "build", "-p", "soundcraft-ui-egui", "--example", "ui_shot"], cwd=ROOT, check=True)
    for name in names:
        images, size, flags, crops = SHOTS[name]
        last = ROOT / "target" / "guide-shot-last.png"
        script = ROOT / "docs/guide/shots" / f"{name}.jsonl"
        for flag, setup in ((FS, "_firestation.jsonl"), (SAMPLE, "_sample.jsonl")):
            if flag in flags:
                project = os.environ.get("FREEDAWM_GUIDE_PROJECT", "")
                if flag == SAMPLE and not (pathlib.Path(project) / "Sample.fdawm").is_file():
                    sys.exit(f"{name}: set FREEDAWM_GUIDE_PROJECT to the folder holding Sample.fdawm")
                merged = ROOT / "target" / "guide-shot-script.jsonl"
                merged.write_text((script.parent / setup).read_text().replace("$PROJECT", project) + script.read_text())
                script = merged
        subprocess.run(
            [ROOT / "target/debug/examples/ui_shot", last, script, "--size", size, "--system-theme", "dark", *(["--blank"] if SAMPLE in flags else []), *(f for f in flags if f not in (FS, SAMPLE))],
            cwd=ROOT,
            check=True,
            stdout=subprocess.DEVNULL,
        )
        folder = ROOT / "docs/guide/images"
        sources = {image: Image.open(folder / f"{image}.png").copy() for image in images}
        for out, source, box in crops:
            sources[source].crop(tuple(2 * v for v in box)).save(folder / f"{out}.png", optimize=True)
        for image in {*images, *(c[0] for c in crops)}:
            print(f"wrote docs/guide/images/{image}.png")
    return 0


if __name__ == "__main__":
    sys.exit(main())
