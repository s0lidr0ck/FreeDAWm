#!/usr/bin/env bash
# Prints a GitHub Release's notes: docs/releases/<tag>.md (what's new) when there is one, then the
# Downloads section the Release workflow has always written.
#
#   packaging/release-notes.sh v0.9.4 <commit sha>
set -euo pipefail
tag="${1:?usage: release-notes.sh <tag> <sha>}"
sha="${2:?usage: release-notes.sh <tag> <sha>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
notes="$root/docs/releases/$tag.md"
if [ -f "$notes" ]; then
  cat "$notes"
  echo
fi
cat <<EOF
## Downloads

Unsigned builds made by GitHub Actions from ${sha::12}.

- **Windows x64**: \`.msi\` installer or \`-portable.zip\`. The files are not code-signed, so SmartScreen may say "Windows protected your PC": choose *More info › Run anyway*. Audio goes through ASIO or WASAPI. This Windows build includes Steinberg's ASIO SDK under GPLv3, so it is distributed under GPLv3 as a whole (see \`ASIO-NOTICE.txt\`); FreeDAWm's source stays MIT OR Apache-2.0.
- **macOS 11+ on Apple silicon** (no Intel build yet): open the \`.dmg\` and drag FreeDAWm to Applications. The app is not notarized: the first time, right-click (Control-click) FreeDAWm in Applications and choose *Open*, then *Open* again. On macOS 15 and later, if there is no *Open* button, go to *System Settings › Privacy & Security* and click *Open Anyway* under the FreeDAWm message (or run \`xattr -dr com.apple.quarantine /Applications/FreeDAWm.app\`).
- **Linux x86_64** (glibc 2.39+: Ubuntu 24.04, Debian 13, Fedora 40 or newer): \`.AppImage\` (\`chmod +x\` and run), \`.deb\`, \`.rpm\` or \`.tar.gz\`.

Check downloads against \`SHA256SUMS.txt\`.
EOF
