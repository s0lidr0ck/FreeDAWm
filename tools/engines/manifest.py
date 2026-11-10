"""Write engines.json, the song engines' downloads manifest, from the engine zips.

    python3 tools/engines/manifest.py <dir with the zips> <out engines.json> [--base-url URL]

Every `<id>-<platform>.zip` in the folder becomes that engine's `platforms` entry (url, size,
sha256); the engine's models (tools/engines/engines.json) become its `fetch` list, downloaded by
FreeDAWm from Hugging Face. The app reads engines.json beside models.json in the models release
(crates/ml/src/catalog.rs: Manifest, ManifestModel::platforms / fetch).
"""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for blk in iter(lambda: f.read(1 << 20), b""):
            h.update(blk)
    return h.hexdigest()


def main(zips: str, out: str, base_url: str = "") -> dict:
    spec = json.loads((HERE / "engines.json").read_text(encoding="utf-8"))["engines"]
    base = base_url.rstrip("/") + "/" if base_url else ""
    models = []
    for e in spec:
        platforms = {}
        for z in sorted(Path(zips).glob(f"{e['id']}-*.zip")):
            platform = z.stem[len(e["id"]) + 1:]
            platforms[platform] = {"url": base + z.name, "size": z.stat().st_size, "sha256": sha256(z)}
        if not platforms:
            continue
        models.append({
            "id": e["id"],
            "name": e["name"],
            "version": e["version"],
            "url": "", "size": 0, "sha256": "",
            "license": "CC BY-NC 4.0 — non-commercial, personal use" if e["personal_use_only"] else "MIT",
            "license_note": e["license"],
            "source": e["source"],
            "personal_use_only": e["personal_use_only"],
            "manual": False,
            "manual_note": "",
            "platforms": platforms,
            "fetch": [{"file": m["file"], "url": m["url"], "size": m["size"], "sha256": m["sha256"]} for m in e["models"]],
        })
    manifest = {"format": 1, "release": "models-v1", "models": models}
    Path(out).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"{out}: " + ", ".join(f"{m['id']} ({', '.join(m['platforms'])})" for m in models))
    return manifest


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    base = sys.argv[sys.argv.index("--base-url") + 1] if "--base-url" in sys.argv else ""
    if base:
        args = [a for a in args if a != base]
    if len(args) != 2:
        raise SystemExit(__doc__)
    main(args[0], args[1], base)
