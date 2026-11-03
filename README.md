<p align="center">
  <img alt="FreeDAWm" src="assets/app-icon/freedawm-1024.png" width="128">
</p>

<h1 align="center">FreeDAWm</h1>

<p align="center">
  <b>Recording, editing and mixing audio; an open-source, modern digital audio workstation (DAW), built in pure Rust.</b>
</p>

<p align="center">
  A complete digital audio workstation that runs natively on macOS, Windows, Linux and FreeBSD, and
  in the browser. Multitrack editing, a full mixer with plugins, sends and automation, MIDI, recording
  and bouncing, and every action scriptable from the command line or by an AI agent.
</p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/100%25-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="egui" src="https://img.shields.io/badge/UI-egui-14a9c4?style=flat-square">
  <img alt="Platforms" src="https://img.shields.io/badge/macOS%20%C2%B7%20Windows%20%C2%B7%20Linux%20%C2%B7%20FreeBSD%20%C2%B7%20Web-0b7385?style=flat-square">
  <img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue?style=flat-square">
  <img alt="Status: pre-alpha" src="https://img.shields.io/badge/status-pre--alpha-orange?style=flat-square">
</p>

<p align="center">
  <a href="https://github.com/s0lidr0ck/FreeDAWm"><b>Source code</b></a> ·
  <a href="https://github.com/s0lidr0ck/FreeDAWm/releases">Releases</a> ·
  <a href="https://github.com/s0lidr0ck/FreeDAWm/issues">Issues</a> ·
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

<p align="center">
  <img alt="FreeDAWm's Edit window with a nine-track session: drums, bass, pad, keys and lead, a reverb aux and a master fader" src="docs/images/edit.png" width="100%">
  <br>
  <sub>The Edit window: tracks, waveforms, MIDI, rulers, markers and the transport, playing the built-in demo song (every sound in it is synthesised by FreeDAWm itself).</sub>
</p>

> [!NOTE]
> FreeDAWm is based on [SoundCraft](https://github.com/storytold/soundcraft) by the ArtCraft
> team (MIT OR Apache-2.0). It is an independent fork, not an ArtCraft product.

<p align="center">
  <a href="#what-it-does">What it does</a> ·
  <a href="#getting-started">Getting started</a> ·
  <a href="#for-agents-and-scripts">Agents &amp; scripts</a> ·
  <a href="#how-its-built">How it's built</a> ·
  <a href="#status-and-roadmap">Status</a> ·
  <a href="#downloads">Downloads</a> ·
  <a href="#license-and-credits">License</a>
</p>

## What it does

FreeDAWm follows the workflow audio engineers already know: an **Edit window** for arranging and
editing, a **Mix window** for balancing, edit modes, tools, playlists, memory locations, offline
processing (the Process menu) and the rest. If you've spent years in that workflow, your hands should already know
where things are.

<table>
  <tr>
    <td width="50%"><img alt="The Mix window with channel strips, inserts, sends, I/O, pan knobs, faders and meters, and an EQ plugin window" src="docs/images/mix.png"></td>
    <td width="50%"><img alt="The MIDI editor showing a lead melody as a piano roll with a velocity lane" src="docs/images/midi.png"></td>
  </tr>
  <tr>
    <td><b>Mix window.</b> Ten inserts and ten sends per track, busses and aux inputs, master faders, VCAs, routing folders, solo-in-place with implicit solo, and meters with peak hold.</td>
    <td><b>MIDI editor.</b> Instrument tracks with built-in synths, a piano roll with a velocity lane, step input, quantize, transpose, and Standard MIDI File import and export.</td>
  </tr>
  <tr>
    <td width="50%"><img alt="Volume automation drawn on the Pad track, with the Memory Locations and Big Counter windows open" src="docs/images/automation.png"></td>
    <td width="50%"><img alt="The FreeDAWm app icon: a bird in flight in a blue-to-orange gradient, with waveform bars in its body" src="assets/app-icon/freedawm-1024.png" width="70%"></td>
  </tr>
  <tr>
    <td><b>Automation and markers.</b> Breakpoint automation for volume, pan, mute, sends and every plugin parameter, written live in Write, Touch and Latch, plus memory locations and a big counter.</td>
    <td><b>Everywhere.</b> Signed builds for macOS (universal), Windows (x64, x86, Arm), Linux (AppImage, deb, rpm, Flatpak), FreeBSD, and a WebAssembly build that runs in the browser.</td>
  </tr>
  <tr>
    <td width="50%"><img alt="The Video window showing a colour-bar test movie with a timecode burn-in, above a Video track with thumbnails in the Edit window" src="docs/images/video.png"></td>
    <td width="50%"><img alt="The Renderer window showing a 7.1.4 speaker layout with two object tracks, beside the Mix window with surround panners and Object/Bed buttons" src="docs/images/surround.png"></td>
  </tr>
  <tr>
    <td><b>Video.</b> A picture track with thumbnails and a Video window that follows the playhead, decoding H.264, ProRes and Motion JPEG in pure Rust, with timecode burn-in and sync offset.</td>
    <td><b>Surround and immersive.</b> Main and bus formats from stereo to 9.1.6 and Ambisonics, a surround panner with divergence and height, object/bed routing and a Renderer window with live speaker levels.</td>
  </tr>
  <tr>
    <td width="50%"><img alt="The Score Editor showing the lead melody in standard notation on a treble and bass staff" src="docs/images/score.png"></td>
    <td></td>
  </tr>
  <tr>
    <td><b>Notation.</b> A Score Editor for MIDI tracks, MusicXML export for Sibelius and other notation programs, and printable engraved scores.</td>
    <td><b>Your plugins too.</b> CLAP, VST3 and Audio Units, with their own editors, their state saved in the session, and every instance created off the audio thread.</td>
  </tr>
</table>

### Feature tour

- **Editing:** Shuffle, Slip, Spot and Grid modes; Smart, Selector, Grabber, Trim, Pencil, Zoom and
  Scrubber tools; cut, copy, paste, duplicate, repeat, shift, insert silence, separate (at
  selection, on grid, at transients), heal, consolidate, strip silence, nudge; fades and
  crossfades in five shapes, drawn straight from the clip corners; playlists with comping lanes;
  clip gain lines; edit groups; single-key Commands Focus shortcuts; trackpad scrolling that
  follows your fingers and pinch-to-zoom around the pointer; undo for everything.
- **Mixing:** fader and pan with automation, pre/post sends, busses, aux inputs, master faders,
  VCA masters, routing folders, solo modes, mute and solo groups, phase invert and trim, clip
  effects, and automatic plugin delay compensation.
- **Automation:** volume, pan, mute, send and every plugin parameter; Write, Touch, Latch and
  Trim passes recorded live from the faders; thin, glide, coalesce and convert to clip gain.
- **Plugins:** 7-band and 1-band EQ, compressor/limiter, expander/gate, de-esser, maximizer,
  channel strip, room and plate reverbs, mod delay, chorus, flanger, phaser, saturator, lo-fi,
  rectifier, pitch shifter, time shift, gain, trim, invert, DC removal, signal generator, dither,
  a subtractive synth, a drum synth and a sampler (plays any WAV, AIFF, FLAC, MP3 or OGG across the
  keyboard), all original. **CLAP plugins** load too. Any processor
  runs offline from the Process menu, and user presets save per plugin.
- **MIDI:** instrument and MIDI tracks, a piano-roll MIDI editor with a velocity lane, an event
  list, a score view, step input, quantize, transpose, real-time properties, Standard MIDI Files,
  and Audio-to-MIDI from a sung or played melody.
- **Recording:** punch in on a selection or on the fly, pre/post-roll, loop recording with a
  playlist per take, input monitoring through the full channel strip, and autosave with recovery.
- **Time and tempo:** Bars|Beats, Min:Secs, Timecode (every rate, drop-frame included),
  Feet+Frames and Samples; tempo and meter maps with a tempo editor; linear, parabolic and
  S-curve tempo ramps; key signatures and detected chords; Beat Detective and Identify Beat;
  pitch-preserving Elastic warping.
- **Audio files:** WAV/BWF/RF64, AIFF/AIFC and FLAC in and out; MP3, Ogg Vorbis, AAC, ALAC, CAF
  and the audio of MP4/MOV movies in. Any sample rate and bit depth.
- **Bounce:** mix or stems to WAV, AIFF or FLAC at 16/24/32-bit float with dither and
  normalisation, reporting peak, true peak and integrated loudness (LUFS).
- **Fast:** a parallel mix engine, copy-on-write undo, and a UI that holds 120 frames per second
  with seventy-odd tracks playing.

## Getting started

Download a build from the [releases page](https://github.com/s0lidr0ck/FreeDAWm/releases), or build
from source with Rust 1.95 or later for the desktop/web UI (the core workspace's declared
minimum remains Rust 1.90):

```sh
git clone https://github.com/s0lidr0ck/FreeDAWm
cd freedawm
cargo run --release -p soundcraft -- --demo     # opens the demo song
```

Linux needs the ALSA headers (`libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel` on Fedora).

The web build:

```sh
cd apps/soundcraft-web && trunk serve --release   # then open http://127.0.0.1:8080
```

Useful shortcuts: <kbd>Space</kbd> play/stop, <kbd>⌘</kbd><kbd>=</kbd> Mix/Edit,
<kbd>F1</kbd>–<kbd>F4</kbd> edit modes, <kbd>F5</kbd>–<kbd>F10</kbd> tools, <kbd>⌘</kbd><kbd>E</kbd>
separate, <kbd>⌘</kbd><kbd>D</kbd> duplicate, <kbd>Enter</kbd> new marker, <kbd>⌘</kbd><kbd>⇧</kbd><kbd>N</kbd>
new tracks. **Setup › Keyboard Shortcuts** lists them all.

### Logs

The desktop app writes its `log` records to standard error and to `logs/freedawm.log` in the
settings directory, beside `ui.json`, `Autosave/` and `Presets/` (Linux `~/.config/freedawm/logs/`,
or `$XDG_CONFIG_HOME/freedawm/logs/`; macOS `~/Library/Application Support/FreeDAWm/logs/`;
Windows `%APPDATA%\FreeDAWm\logs\`). A start from a desktop menu or the Dock has no terminal, so
this file is what to attach to a bug report: the engine's panic report, audio devices that failed to
open or broke, plugins that refused their stored state, CLAP plugins' own messages and files that
failed to open land there. Each launch moves the previous log to `freedawm.1.log` (and that one to
`freedawm.2.log`), so the log of a run that crashed survives the next start. The file stops
growing at 16 MiB. `--version` writes no file, and runs with `FREEDAWM_NO_PREFS` log to standard
error only.

| Variable | Effect |
|---|---|
| `RUST_LOG` | Log levels for standard error and the log file. Default: `info` for FreeDAWm's own crates, `warn` for everything else. env_logger-style directives replace that, e.g. `RUST_LOG=debug`, `RUST_LOG=warn,soundcraft_mix=trace` or `RUST_LOG=info,wgpu_core=warn`; a directive ending in `*` covers every target starting with it (`soundcraft*=debug`: the crates keep their `soundcraft-*` names). |
| `FREEDAWM_NO_PREFS` | No preferences read or written and no log file (agents' test runs). |
| `FREEDAWM_NO_SPLASH` | No launch splash (also `--no-splash`). The splash is never shown for `--headless`, `--control` / `FREEDAWM_CONTROL_PORT` (control-channel and MCP launches), `FREEDAWM_NO_PREFS`, `ui_shot` or `CI`. |

### Launch splash

The desktop app opens with a six-second animation (the bird settling into the logo) in a
borderless window centred on screen while it loads; a click or Esc skips it. **Window › Appearance ›
Show splash screen at launch** turns it off (on by default; kept in `ui.json`). Automation runs
never show it (see the table above). The frames are made once from
`assets/splash/freedawm-splash-source.mp4` by `python packaging/splash.py` (needs ffmpeg) and
committed, so builds need no video tools.

### Renamed from SoundCraft

The app was called SoundCraft. On the first start after the rename, FreeDAWm copies the old
settings folder (`%APPDATA%\SoundCraft`, `~/Library/Application Support/SoundCraft`,
`~/.config/soundcraft`, or the portable `SoundCraftData`) into the new one when only the old one
exists, logs one line, and leaves the old folder as a backup; it never overwrites a FreeDAWm
folder that already exists. The ML model folder (`%LOCALAPPDATA%\SoundCraft\models`) is not
copied (models are large): it is used as it is until a `FreeDAWm\models` folder exists.
Every `FREEDAWM_*` environment variable also reads its old `SOUNDCRAFT_*` name when the new one is
unset. Sessions are now saved as `.fdawm` (bundles `.fdawmz`); `.scraft` and `.scraftz` files
still open, and save back to the same file. The programs are `freedawm`, `freedawm-cli` and
`freedawm-scan`: update MCP client configs and scripts that name `soundcraft` or
`soundcraft-cli`.

The realtime audio thread never writes a record itself: the logger keeps the first one it logs
(without waiting or allocating), counts the rest, and the UI thread writes them. The logger is
`apps/soundcraft/src/logging.rs`. The web app logs to the browser console instead.

## For agents and scripts

Everything you can click is also a command with an id and JSON parameters, and the same commands
are reachable from the menus, the keyboard, the command line, a JSON control channel and an
[MCP](https://modelcontextprotocol.io) server. Programmatic calls never open dialogs, and
`session.inspect` reports exactly what changed, so an agent can check its own work.

```sh
# Headless: load the demo, turn the kick down, add an EQ, bounce.
freedawm-cli run --demo \
  --cmd 'mix.volume={"track":"Kick","db":-6}' \
  --cmd 'mix.insert={"track":"Bass","plugin":"eq_7band"}' \
  --bounce mix.wav

# Drive the running app.
freedawm --demo --control 7801 &
freedawm-cli app --port 7801 transport.play
freedawm-cli app --port 7801 ui.screenshot '{"path":"shot.png"}'

# Give Claude a DAW.
claude mcp add freedawm -- freedawm-cli mcp --demo

# Or keep the agent to some folders (reads and writes are separate roots).
freedawm-cli mcp --automation-read-root DIR --automation-write-root DIR
```

See [`docs/control-protocol.md`](docs/control-protocol.md) and [`docs/mcp.md`](docs/mcp.md).
`freedawm-cli commands` lists every command.

Open **FreeDAWm › Session Audio Health** to check loaded audio availability, sample-rate
mismatches and clip source bounds across all playlists, including alternate takes. The report
offers a refresh button and suggests how to resolve each issue. Scripts can obtain the same JSON
report with `freedawm-cli run --demo --cmd 'session.audio_health={}'`. This checks loaded media,
not files on disk, signal levels, plugins or routing.

## How it's built

FreeDAWm is a Cargo workspace of small crates with strict layering: nothing below the UI knows
about egui, so the interface could be swapped for another one.

| Crate | What it does |
|---|---|
| `soundcraft-time` | Timebases, tempo and meter maps, timecode, grid |
| `soundcraft-audio-io` | Audio file formats and waveform overviews |
| `soundcraft-midi` | Standard MIDI Files and MIDI operations |
| `soundcraft-dsp` | Plugins, offline processing, meters, loudness, FFT |
| `soundcraft-model` | The session document |
| `soundcraft-mix` | The mix engine (realtime and offline share it) |
| `soundcraft-engine` | Commands, undo, editing, import/export, bounce |
| `soundcraft-playback` | Audio devices (CoreAudio, WASAPI, ALSA, WebAudio) and recording |
| `soundcraft-automation` | MCP server and control-channel client |
| `soundcraft-ui-egui` | The user interface |

The code never panics on bad input: errors are values, and a malformed file or a bad agent call
produces a message, never a crash. `cargo xtask ci` runs formatting, clippy, the tests, the asset
and layering checks and the WebAssembly build.

## Status and roadmap

FreeDAWm is **pre-alpha**: record, edit, mix and bounce work end to end, but choosing an audio
interface and buffer size, recording MIDI from a controller and session interchange (AAF/OMF) are
still missing, and those are the alpha checklist. Translations are missing too. The status, the
stage and our effort estimates are in [`ROADMAP.md`](ROADMAP.md); the ranked list of what's
missing is in [`docs/gaps.md`](docs/gaps.md). Bug reports and wish lists are very welcome in the
issues.

## Downloads

**Download FreeDAWm** from GitHub: the [latest release](https://github.com/s0lidr0ck/FreeDAWm/releases/latest) has every build listed below, and [all releases](https://github.com/s0lidr0ck/FreeDAWm/releases) have earlier versions and their notes. `<ver>` in the file names is the version number, and `SHA256SUMS.txt` lists a checksum for every file.

### Windows

| Build | Installer | Portable |
|---|---|---|
| x64 (64-bit Intel/AMD) | `freedawm-<ver>-windows-x64.msi` | `freedawm-<ver>-windows-x64-portable.zip` |
| arm64 (Snapdragon and other ARM PCs) | `freedawm-<ver>-windows-arm64.msi` | `freedawm-<ver>-windows-arm64-portable.zip` |
| x86 (32-bit) | `freedawm-<ver>-windows-x86.msi` | `freedawm-<ver>-windows-x86-portable.zip` |

Installers and executables are code-signed.

**If the app doesn't open on Windows:** the desktop app initializes only DirectX 12 by default.
Letting wgpu also create an OpenGL instance can crash some graphics drivers (AMD's
`atio6axx.dll`) before the window appears, so the app would flash in Task Manager and quit.
`WGPU_BACKEND` overrides the default for troubleshooting (for example `dx12` or `vulkan`). In
PowerShell, from the folder containing the executable:

```powershell
$env:WGPU_BACKEND = "vulkan"
& .\freedawm.exe
Remove-Item Env:WGPU_BACKEND                     # restore the default for later launches
```

An explicit `gl` override can bring the driver crash back on affected systems. The macOS, Linux
and web backend defaults are unchanged.

DirectX 12 shaders are compiled with FXC, which is part of Windows, so no `dxcompiler.dll` or
`dxil.dll` is needed ("Parent device is lost" at start, #185). `$env:WGPU_DX12_COMPILER = "dxc"`
(or `"auto"`) selects wgpu's DXC handling again for troubleshooting.

**If the graphics driver resets** (a TDR, "The GPU device instance has been suspended",
`0x887A0005`): FreeDAWm autosaves the session, closes its plugin editors and opens a new window
with a new graphics device; the session, playback and the audio device carry on, and the status
line says so. If a new device can't be made after several tries, the session is saved, a message
says so and FreeDAWm restarts (`--gpu-restarted`), offering the unsaved work. After three resets
within ten minutes it saves, says so and closes instead. The log names the adapter and driver.
To try it: `freedawm.exe --simulate-device-loss` (the device is destroyed 5 s after the window
opens; `=SECS` sets the delay, `=SECS,repeat` loses every new window's device as well, which
reaches the give-up message).

### macOS

| Build | File | Notes |
|---|---|---|
| App, universal (Apple silicon + Intel) | `freedawm-<ver>-macos-universal.dmg` | Signed and notarized |
| Command-line tool, universal | `freedawm-cli-<ver>-macos-universal.zip` | Signed and notarized |

### Linux

| Format | x86_64 | aarch64 (ARM64) | Notes |
|---|---|---|---|
| AppImage | `freedawm-<ver>-linux-x86_64.AppImage` | `freedawm-<ver>-linux-aarch64.AppImage` | Runs anywhere; updates itself with [AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate) (`.zsync` files) |
| Flatpak | `freedawm-<ver>-linux-x86_64.flatpak` | `freedawm-<ver>-linux-aarch64.flatpak` | Sandboxed; `flatpak install --user <file>` |
| Debian/Ubuntu | `freedawm-<ver>-linux-x86_64.deb` | `freedawm-<ver>-linux-aarch64.deb` | |
| Fedora/RHEL/openSUSE | `freedawm-<ver>-linux-x86_64.rpm` | `freedawm-<ver>-linux-aarch64.rpm` | |
| Tarball | `freedawm-<ver>-linux-x86_64.tar.gz` | `freedawm-<ver>-linux-aarch64.tar.gz` | Unpack anywhere |

RISC-V (riscv64): `freedawm-<ver>-linux-riscv64.tar.gz`, a tarball only (glibc 2.39+, e.g. Ubuntu 24.04).

### FreeBSD

| Build | File |
|---|---|
| x86_64 | `freedawm-<ver>-freebsd-x86_64.tar.gz` |

### Web (WebAssembly)

| Build | File | Notes |
|---|---|---|
| Static site | `soundcraft-web-<ver>.zip` | Runs in a modern browser; host it on any static server |

## License and credits

FreeDAWm is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Copyright (c) 2026 FireStation (the FreeDAWm owner) and the FreeDAWm contributors; based on
SoundCraft, copyright (c) 2026 ArtCraft Team and the SoundCraft contributors. Required notices
are in [NOTICE](NOTICE).

Bundled fonts, icons, images and other assets keep their own open licenses; each one is listed
with its author, source and license in [ATTRIBUTION.md](ATTRIBUTION.md). The FreeDAWm logo,
wordmark and splash, the FireStation presets, the Band library and the amp impulse responses are
the owner's own work, released under MIT.

Every icon in the interface is drawn in code, the app icon is original, and the demo song and the
screenshots' audio are synthesised by FreeDAWm itself, so there are no samples or artwork from
anyone else in this repository.

**Trademarks.** Pro Tools, AudioSuite and Avid are trademarks of Avid Technology, Inc. Hardware
named in some modes, presets and impulse responses (1176, LA-2A, SSL, Neve, Pultec, Fender,
Marshall and others) is named only to identify the products studied, and those names are
trademarks of their respective owners. FreeDAWm is not affiliated with or endorsed by any of them;
see [NOTICE](NOTICE).

Tracks and Groups, Clips, MIDI Editor, Memory Locations, and Undo History use movable panels. Edit and Mix retain separate arrangements; Clips and MIDI Editor belong to Edit. Transport, plug-in, and specialist utility windows retain their existing hosts. Drag a panel header to a target edge or another header. Its context menu offers Float panel, Move to group, and Close panel. Existing panel toggles reopen panels at their saved location; Reset Panel Layout restores the app layout. Floating panels remain inside the current app window.
