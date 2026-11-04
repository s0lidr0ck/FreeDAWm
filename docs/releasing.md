# Releasing FreeDAWm

`.github/workflows/release.yml` builds FreeDAWm on GitHub's free standard runners and attaches
the packages to a GitHub Release. The builds are **unsigned** (no Apple Developer ID, no Windows
code-signing certificate) and the Windows build has **no ASIO** (WASAPI only; see
[release-playbook.md](release-playbook.md) › ASIO). It uses only `GITHUB_TOKEN`.

This is FreeDAWm's implementation of the shared [release playbook](release-playbook.md).
User-facing names say **FreeDAWm**. Files, binaries and ids stay lowercase
(`freedawm-<version>-<platform>-<arch>.<ext>`, `io.github.s0lidr0ck.FreeDAWm`).

## Cutting a release

1. **Bump the version** on `main`. The only place it lives is `[workspace.package] version` in
   the root `Cargo.toml`; the app reads it from `CARGO_PKG_VERSION` (*About FreeDAWm*,
   `freedawm --version`, `freedawm-cli --version`).

   ```sh
   cargo xtask version                 # prints the current version, e.g. 0.1.0
   cargo xtask version set 0.2.0       # or 0.2.0-rc.1; updates Cargo.toml and Cargo.lock
   ```

   Commit the change (`Cargo.toml` + `Cargo.lock`) through the normal review flow.
2. **Optional dry run:** *Actions › Release › Run workflow* on `main` builds everything and
   creates (or updates) a **draft** release `v0.2.0` with every package and `SHA256SUMS.txt`.
   Nobody sees a draft until a maintainer publishes it; download a package or two and try them.
3. **Tag and push:** `git tag v0.2.0 && git push origin v0.2.0`. The workflow checks that the tag
   matches `Cargo.toml`, builds everything (about 30 to 60 minutes) and publishes the release
   (reusing the draft from step 2 if there is one). A version with a pre-release suffix
   (`-rc.1`) is published as a pre-release.

Once a version is published the workflow refuses to touch it again, so bump the version first.
Pull requests that change `packaging/`, the app icons or `release.yml` run the build and packaging
jobs as a check, without making a release.

## What gets built

| Platform | Artifacts | Built on |
|---|---|---|
| macOS 11+, Apple silicon | `freedawm-<v>-macos-aarch64.dmg`, `freedawm-cli-<v>-macos-aarch64.zip` (ad-hoc signed, not notarized) | `macos-15` |
| Windows 10+ x64 | `freedawm-<v>-windows-x64.msi`, `freedawm-<v>-windows-x64-portable.zip` (unsigned) | `windows-latest` |
| Linux x86_64 (glibc 2.39+) | `freedawm-<v>-linux-x86_64.{AppImage,deb,rpm,tar.gz}`, `.AppImage.zsync` | `ubuntu-24.04` |

No Intel Mac build: ONNX Runtime (the `ort` crate's prebuilt library, used by the ML features)
has no `x86_64-apple-darwin` binaries. Linux builds on Ubuntu 24.04 because that library needs
the C++ runtime of GCC 13 or newer. The x86, ARM64, aarch64 and riscv64 builds, Flatpak,
FreeBSD and the web build that the upstream pipeline made are not part of the free workflow;
their scripts below still work by hand.

### First launch of unsigned builds

- **macOS:** the app is ad-hoc signed and not notarized, so Gatekeeper stops the first launch.
  Drag FreeDAWm to Applications, then **right-click (Control-click) it › Open › Open**. On
  macOS 15 and later, if the dialog has no *Open* button: *System Settings › Privacy & Security*,
  scroll to the FreeDAWm message and click **Open Anyway**. (Or, in Terminal:
  `xattr -dr com.apple.quarantine /Applications/FreeDAWm.app`.) Later launches work normally.
- **Windows:** SmartScreen may show "Windows protected your PC": click **More info › Run anyway**.
- **Linux:** `chmod +x freedawm-*.AppImage` and run it, or install the `.deb` / `.rpm`.

The from-source Flatpak manifest (`packaging/linux/flatpak/io.github.s0lidr0ck.FreeDAWm.yml`) is
kept ready for a Flathub submission; the release's `.flatpak` bundles come from
`io.github.s0lidr0ck.FreeDAWm.bundle.yml` (see Linux below).

### Audio libraries

FreeDAWm plays and records through [cpal](https://github.com/RustAudio/cpal): CoreAudio on
macOS, WASAPI on Windows, ALSA on Linux and FreeBSD (PulseAudio and PipeWire through their ALSA
plugins), and WebAudio in the browser.

- **Linux** builds need the ALSA headers (`libasound2-dev` and `pkg-config` on Ubuntu). The binary
  links `libasound.so.2`, so the `.deb` depends on `libasound2 | libasound2t64` and the `.rpm` on
  `alsa-lib`; the AppImage and tarball expect it on the host (every desktop distro has it).
- **FreeBSD** builds need `alsa-lib` (`pkg install alsa-lib`); at runtime `alsa-plugins-oss`
  routes ALSA to the OSS `sound(4)` devices. Without a device the player falls back to its
  silent clock, so the transport still runs.
- **Flatpak**: alsa-lib and its PulseAudio plugin come with the freedesktop runtime; the
  manifests grant `--socket=pulseaudio` (PipeWire's pulse server answers on it). No raw ALSA
  devices and no JACK: FreeDAWm doesn't use either.
- **Web**: browsers start audio only after a user gesture, so the first click in the page may be
  needed before sound plays. The web build has no recording.

### macOS

`packaging/macos/package.sh` builds `aarch64-apple-darwin` (and, with `--arch universal`,
`x86_64-apple-darwin` too, joined with `lipo`; that needs an ONNX Runtime for Intel Macs, which
`ort` does not ship) with `MACOSX_DEPLOYMENT_TARGET=11.0`, and assembles `FreeDAWm.app`:

- `Info.plist` is generated from `Info.plist.in`. The bundle id is `io.github.s0lidr0ck.FreeDAWm`.
  It declares the session type for `.fdawm` and the older `.scraft`
  (`io.github.s0lidr0ck.FreeDAWm.session`, MIME `application/x-freedawm-session` and
  `application/x-soundcraft-session`, role Editor, rank Owner) and lists WAV, AIFF/AIFC, FLAC,
  MP3 and MIDI as Viewer/Alternate, so FreeDAWm appears under *Open With* without taking over
  the default player. `NSMicrophoneUsageDescription` explains the recording prompt.
- **Signing** goes inside-out with the hardened runtime and a secure timestamp. The executable is
  signed first, then the bundle. There's no `--deep` on the final signature. `entitlements.plist`
  holds a single exception, `com.apple.security.device.audio-input`, without which the hardened
  runtime blocks recording.
- **Notarization:** the app is zipped and sent with `xcrun notarytool submit --wait`, then the
  ticket is stapled to the app. The app goes on a DMG (`hdiutil`, with an `Applications` link).
  The DMG is signed, notarized and stapled too.
  Its Finder window (background, icon size and positions) comes from
  [`packaging/macos/dmg/`](../packaging/macos/dmg/README.md), and its volume is named `FreeDAWm`
  without the version, which the window's background needs; the DMG file name keeps the version.
- **CLI:** `freedawm-cli` is signed with the hardened runtime, zipped and
  notarized.

Locally, without certificates, the script signs ad-hoc (`codesign -s -`) and skips notarization:

```sh
packaging/macos/package.sh --arch aarch64     # what CI builds
open dist/release/freedawm-*-macos-*.dmg
```

### Windows

`packaging/windows/package.ps1 -Arch x64|x86|arm64` builds with the dynamic C runtime (ONNX
Runtime's prebuilt library is compiled against it, so `+crt-static` does not link) and stages
`DirectML.dll` and the Visual C++ runtime DLLs (from Visual Studio's `VC\Redist`) next to the
programs, so neither the MSI nor the portable zip needs the VC++ redistributable installed. CI
builds x64 only, without the `asio` feature.

- `apps/soundcraft/build.rs` embeds the icon (`assets/app-icon/freedawm.ico`) and VERSIONINFO
  with `winresource`, only when targeting Windows. Release packaging sets
  `FREEDAWM_REQUIRE_WINRES=1` so a missing resource compiler fails the build.
- The script checks both PE headers: the machine type matches `-Arch`, `freedawm.exe` is a GUI
  app (no console window) and `freedawm-cli.exe` a console app.
- `freedawm.wxs` (WiX v5) is a per-machine install into Program Files with an advertised Start
  Menu shortcut. FreeDAWm becomes the default app for `.fdawm` (and the older `.scraft`) and is listed under *Open with*
  for `.wav`, `.aif`, `.aiff`, `.flac`, `.mp3`, `.mid` and `.midi`. It also registers App Paths
  (Win+R `freedawm`). The MSI version is the numeric `X.Y.Z`; same-version upgrades let release
  candidates replace each other. The upgrade code `EE53D3E5-D0AE-40F9-808D-9417FDEBF61A` is fixed
  forever.
- **Signing:** `packaging/windows/sign.ps1` signs both `.exe` files and then the `.msi` with
  `signtool` (SHA-256, RFC 3161 timestamp), using a `.pfx` (`WINDOWS_CERTIFICATE*`) if present,
  otherwise Azure Trusted Signing (all six `AZURE_*`), otherwise nothing (a warning).

Locally on Windows: `dotnet tool install -g wix --version 5.0.2`, then
`pwsh packaging/windows/package.ps1 -Arch x64`.

### Linux

`packaging/linux/package.sh` stages one FHS tree and makes every format from it: both binaries,
`io.github.s0lidr0ck.FreeDAWm.desktop` (with `MimeType=` for sessions, WAV, AIFF, FLAC, MP3 and
MIDI), hicolor icons from 16 px to 512 px (PNG only; no scalable SVG), AppStream metainfo, and a
shared-mime-info file declaring `application/x-freedawm-session` (`*.fdawm`) and the older
`application/x-soundcraft-session` (`*.scraft`).

- **AppImage** (maintained `AppImage/appimagetool`, static runtime: no libfuse2 needed). Each
  embeds the update information
  `gh-releases-zsync|freedawm|freedawm|latest|freedawm-*-linux-<arch>.AppImage.zsync`, and
  the `.zsync` next to it (written when `zsyncmake` from the `zsync` package is installed, as in
  CI) lets AppImageUpdate fetch only the changed blocks of the newest non-pre-release.
- **.deb** and **.rpm** by [nfpm](https://nfpm.goreleaser.com/) from one `nfpm.yaml`; the
  `postinst.sh` hook refreshes the desktop, MIME and icon caches.
- **.tar.gz** for people who manage their own `/opt` or `~/.local`.
- **Flatpak**: the `flatpak` job runs `packaging/linux/flatpak-bundle.sh` on the Linux job's
  tarball (no Rust build) with `flatpak/io.github.s0lidr0ck.FreeDAWm.bundle.yml`, then installs the
  bundle and runs `freedawm-cli --version` in the sandbox. Install with
  `flatpak install --user freedawm-<v>-linux-<arch>.flatpak`. Both manifests use the freedesktop
  25.08 runtime with Wayland/X11, `dri`, PulseAudio and `xdg-music`; other files go through the
  portals. For the VST3 and CLAP hosts they grant `~/.vst3` and `~/.clap` read-only and mount
  Flathub's `org.freedesktop.LinuxAudio.Plugins` extensions (pointed to by `VST3_PATH` and
  `CLAP_PATH`); system plugin folders aren't visible in the sandbox.
  `io.github.s0lidr0ck.FreeDAWm.yml` builds from source for Flathub and needs vendored crates
  (`cargo-sources.json` from `flatpak-cargo-generator.py`); the commands are in its header.

Binaries are built on Ubuntu 24.04 and need **glibc ≥ 2.39** and GCC 13's `libstdc++` (ONNX
Runtime's prebuilt library). X11, Wayland, xkbcommon, Vulkan
and EGL are loaded at runtime, so the packages declare them as dependencies or recommendations.

Locally (on Linux): install nfpm, then `packaging/linux/package.sh` (or `--formats "deb tar"`).

### FreeBSD

GitHub has no FreeBSD runners, so the `freebsd` job builds inside a FreeBSD 14.3 VM
(`vmactions/freebsd-vm`, pinned by commit) and runs `packaging/freebsd/package.sh`, which stages
a `/usr/local`-style tree (binaries, desktop entry, MIME type, metainfo, icons, docs) as a
`.tar.gz`. Install with `tar -xzf freedawm-*-freebsd-x86_64.tar.gz --strip-components 1 -C /usr/local`.
`.github/workflows/freebsd.yml` builds and tests the workspace there on every push to `main`.

### Web

`packaging/web/package.sh` runs `trunk build --release` in `apps/soundcraft-web` (output in
`dist/web`, `public_url = "./"`) and zips the site with sample `_headers` and `.htaccess` files and
the hosting guide. `apps/soundcraft-web` runs the same `SoundApp` as the desktop app on the
synthesised demo session, rendering with wgpu (WebGPU, or WebGL2 with `?webgl`) and playing
through WebAudio.

## Signing (not set up)

The public workflow signs nothing and needs no secrets. The packaging scripts still support
signing when the material exists, if the owner ever buys it:

- **macOS:** `packaging/macos/package.sh` signs with `MACOS_SIGN_IDENTITY` (hardened runtime,
  secure timestamp) and notarizes when `APPLE_ID`, `APPLE_PASSWORD` (app-specific password) and
  `APPLE_TEAM_ID` are set; `packaging/macos/import-cert.sh` imports a base64 `.p12` into a CI
  keychain. Needs an Apple Developer Program membership.
- **Windows:** `packaging/windows/sign.ps1` signs with a `.pfx` (`WINDOWS_CERTIFICATE`,
  `WINDOWS_CERTIFICATE_PASSWORD`) or Azure Trusted Signing (`AZURE_TENANT_ID`, `AZURE_CLIENT_ID`,
  `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE`),
  and otherwise warns and leaves the files unsigned.

Wiring either into `release.yml` means passing those as secrets to the macOS / Windows job
(ideally through a protected `release` environment). `GITHUB_TOKEN` creates the release; only
the final job gets `contents: write`.

## Icons

`assets/app-icon/freedawm-logo-source.png` is the canonical icon: the owner's FreeDAWm bird (a
transparent 1536 px PNG); see `assets/app-icon/README.md`. `packaging/icons.sh` (Python + Pillow,
`packaging/icons.py`) regenerates the 1024 px PNG, the macOS 512 px window icon, the `.icns`, the
`.ico` and the hicolor PNGs from it, plus the About box and menu bar art in
`crates/ui-egui/assets/brand/`. The outputs are committed, so packaging never needs those tools.
The DMG window art (`packaging/macos/dmg/background.tiff`, `dmg-layout.DS_Store`) still shows the
old nightingale and names the old volume: regenerate it with `packaging/macos/dmg/generate.py`
(needs `resvg`, `ds_store`, `mac_alias`) before the next macOS release. `apps/soundcraft/src/main.rs` sets the runtime window/Dock/taskbar icon from them.

## Checks

`.github/workflows/ci.yml` runs `cargo xtask ci` on Linux and `cargo test --workspace` on macOS
and Windows for every pull request. `release.yml` also runs (build and package, no release) on
pull requests that change `packaging/`, the app icons or the workflow. Both pin the Rust
toolchain (`RUST_TOOLCHAIN`), so a new stable release's lints can't break an unchanged tree.
On Windows, CI skips the DSP `silence_tail_is_cheap` timing tests: on some hosted runners the
silent tail runs 4 to 10 times slower than the signal (likely denormal arithmetic on those
CPUs), which deserves a look on Intel machines. The `soundcraft-playback` and
`soundcraft-ui-egui` test binaries crash with an access violation on those runners (not yet
diagnosed; they have no audio devices), so there they are built but not run.
