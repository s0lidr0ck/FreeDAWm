"""Pack a built song-engine server into FreeDAWm's engine zip.

    python3 tools/engines/package.py <engine id> <platform> <build dir> <upstream source dir> <out dir>

<platform> is FreeDAWm's name for it (`windows-x86_64`, `linux-x86_64`, `macos-aarch64`, as
`catalog::current_platform` says). From <build dir> it takes the server program (the spec's
`server`, `.exe` on Windows) and every shared library beside it (ggml and its backends, built with
GGML_BACKEND_DL so the GPU backend that works is loaded and the CPU one is the fallback). The zip
holds, at its root:

    card.json        id, name, version, task "song_server", file (the server), extra_files (every
                     program file and every model with its sha256), server_args, device_arg,
                     cpu_device, license, source, platform, upstream commit
    bin/...          the server and its libraries
    LICENSE-server   the upstream licence (MIT)

Entries are stored, not compressed: the app reads stored zips only (as ml-publish writes them).

The models are not in the zip: FreeDAWm fetches them from Hugging Face (engines.json `fetch`),
and the card's extra_files lists them so the installed folder is checked as one.
Writes <out dir>/<id>-<platform>.zip and prints its size and sha256 as JSON.
"""

from __future__ import annotations

import hashlib
import json
import sys
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
LIB_SUFFIXES = (".dll", ".so", ".dylib")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for blk in iter(lambda: f.read(1 << 20), b""):
            h.update(blk)
    return h.hexdigest()


def spec(engine_id: str) -> dict:
    for e in json.loads((HERE / "engines.json").read_text(encoding="utf-8"))["engines"]:
        if e["id"] == engine_id:
            return e
    raise SystemExit(f"no engine `{engine_id}` in engines.json")


def program_files(build: Path, server: str, windows: bool) -> list[Path]:
    exe = build / (server + (".exe" if windows else ""))
    if not exe.is_file():
        # Multi-config generators put it under Release/.
        alt = build / "Release" / exe.name
        if alt.is_file():
            build, exe = alt.parent, alt
        else:
            raise SystemExit(f"{exe} is not built")
    libs = sorted(p for p in build.iterdir() if p.is_file() and (p.suffix in LIB_SUFFIXES or ".so." in p.name))
    return [exe] + libs


def main(engine_id: str, platform: str, build_dir: str, source_dir: str, out_dir: str) -> dict:
    e = spec(engine_id)
    windows = platform.startswith("windows")
    files = program_files(Path(build_dir), e["server"], windows)
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    zpath = out / f"{engine_id}-{platform}.zip"
    exe_rel = f"bin/{files[0].name}"
    extra = [{"file": f"bin/{p.name}", "sha256": sha256(p)} for p in files[1:]]
    extra += [{"file": m["file"], "sha256": m["sha256"]} for m in e["models"]]
    card = {
        "id": engine_id,
        "name": e["name"],
        "version": e["version"],
        "task": "song_server",
        "file": exe_rel,
        "sha256": sha256(files[0]),
        "size_bytes": files[0].stat().st_size,
        "extra_files": extra,
        "server_args": e["server_args"],
        "device_arg": e["device_arg"],
        "cpu_device": e["cpu_device"],
        "license": e["license"],
        "source": e["source"],
        "platform": platform,
        "upstream": {"repo": e["repo"], "commit": e["commit"]},
    }
    licence = next((Path(source_dir) / n for n in ("LICENSE", "LICENSE.md", "LICENSE.txt") if (Path(source_dir) / n).is_file()), None)
    with zipfile.ZipFile(zpath, "w", compression=zipfile.ZIP_STORED) as z:
        z.writestr("card.json", json.dumps(card, indent=2) + "\n")
        for p in files:
            z.write(p, f"bin/{p.name}")
        if licence is not None:
            z.write(licence, "LICENSE-server")
    info = {"id": engine_id, "platform": platform, "file": zpath.name, "size": zpath.stat().st_size, "sha256": sha256(zpath)}
    print(json.dumps(info))
    return info


if __name__ == "__main__":
    if len(sys.argv) != 6:
        raise SystemExit(__doc__)
    main(*sys.argv[1:6])
