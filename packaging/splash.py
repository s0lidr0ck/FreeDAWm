#!/usr/bin/env python3
"""Regenerate the launch splash frames from the owner's animation,
assets/splash/freedawm-splash-source.mp4 (1280x720, 24 fps, 6.0 s; its audio track is ignored).

The app has no video decoder, so the film is committed as JPEG frames that the binary embeds
(apps/soundcraft/build.rs lists them; apps/soundcraft/src/splash.rs plays them). Needs ffmpeg on
PATH, or in FFMPEG, or the winget install. The outputs are committed, so builds never need it.
Run from anywhere:

    python packaging/splash.py [--quality N]

Writes assets/splash/frames/f000.jpg … f143.jpg: 960x540, 24 fps (144 frames), JPEG quality
`-q:v 2` (about 3 MB in all). Old frames are removed first, so the count always matches the film.
"""
import argparse
import glob
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "assets" / "splash" / "freedawm-splash-source.mp4"
FRAMES = ROOT / "assets" / "splash" / "frames"
WIDTH, HEIGHT, FPS = 960, 540, 24
WINGET = glob.glob(
    os.path.expandvars(r"%LOCALAPPDATA%\Microsoft\WinGet\Packages\Gyan.FFmpeg*\ffmpeg-*\bin\ffmpeg.exe")
)


def ffmpeg() -> str:
    for candidate in [os.environ.get("FFMPEG"), shutil.which("ffmpeg"), *WINGET]:
        if candidate and Path(candidate).is_file():
            return candidate
    sys.exit("ffmpeg not found: install it (winget install Gyan.FFmpeg) or set FFMPEG")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--quality", type=int, default=2, help="ffmpeg -q:v for the JPEGs (2 best … 31 worst; default 2)")
    args = ap.parse_args()
    if not SOURCE.is_file():
        sys.exit(f"missing {SOURCE}")
    if FRAMES.is_dir():
        shutil.rmtree(FRAMES)
    FRAMES.mkdir(parents=True)
    cmd = [
        ffmpeg(), "-v", "error", "-y", "-i", str(SOURCE), "-an",
        "-vf", f"fps={FPS},scale={WIDTH}:{HEIGHT}:flags=lanczos",
        "-q:v", str(args.quality), str(FRAMES / "f%03d.jpg"),
    ]
    subprocess.run(cmd, check=True)
    files = sorted(FRAMES.glob("f*.jpg"))
    # ffmpeg numbers from 1; the app indexes from 0.
    for i, f in enumerate(files):
        f.rename(FRAMES / f"f{i:03d}.jpg")
    files = sorted(FRAMES.glob("f*.jpg"))
    total = sum(f.stat().st_size for f in files)
    print(f"{len(files)} frames, {WIDTH}x{HEIGHT} @ {FPS} fps, {total / 1e6:.2f} MB in {FRAMES}")


if __name__ == "__main__":
    main()
