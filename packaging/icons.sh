#!/usr/bin/env bash
# Regenerate every app icon from the owner's logo (assets/app-icon/freedawm-logo-source.png):
# freedawm-1024.png, freedawm-macos-512.png, freedawm.ico, freedawm.icns and the hicolor theme.
#
# Needs Python 3 with Pillow (pip install pillow); see packaging/icons.py. The outputs are
# committed, so builds and packaging never need these tools.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "${PYTHON:-python3}" "$ROOT/packaging/icons.py"
