# Architecture

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-11-21 · **Change:** minor (sources play from disk; previously minor: the monitor stage) · **Target:** Avid Pro Tools Ultimate 2026.4.1

FreeDAWm is a Cargo workspace of small crates with enforced layering (`cargo xtask layers`).
Nothing below the UI knows about egui, so the interface can be replaced.

The UI package pins `craft-ui` for common icon-button and track record-enable interactions.
FreeDAWm supplies each state's colors, border, radius and icon painter; selection and command
dispatch remain in the toolbar and track controls. The single-shape frame option preserves the
existing fill/border blending when controls are disabled. Specialized transport Play/Stop/Record styling,
nonlinear meters, faders, pan gestures, fonts and the live theme resolver remain local. Shared adapters read the current palette. The UI
requires Rust 1.95 or later without raising the core workspace's Rust 1.90 declaration.

```
L0  time        audio-io        midi        video   (standalone: no workspace deps)
L1  dsp         clap-host   vst3-host   au-host     (plugins; the three hosts are the only crates
                                                     allowed `unsafe`, confined to their FFI modules)
L2  model                                       (the session document)
L3  mix                                         (the mix engine)
L4  engine      playback                        (commands, undo, I/O · audio devices)
L5  automation                                  (MCP server, control-channel client)
L6  ui-egui                                     (the user interface)
    apps: freedawm (desktop) · freedawm-cli
```

## The document

`soundcraft_model::Session` is plain data (serde): tracks with playlists of clips, mixer state
(inserts, sends, routing, automation lanes), tempo and meter maps, memory locations, groups,
busses, and `EditState` (selection, tools, modes, view flags). Each source's audio lives in a
`SourcePool` of `Arc`s that is not serialised; cloning a session is cheap. Sessions save as
`.fdawm` JSON plus an `Audio Files/` folder.

Sources play from disk ([`model::disk`](../crates/model/src/disk.rs)): a WAV at the session rate
is opened by its header and read through one process-wide block cache (pages of 32768 frames of
one channel; a quarter of RAM, 512 MiB to 6 GiB, `FREEDAWM_DISK_CACHE_MB`). The audio thread only
looks pages up (atomics, no lock, allocation or I/O): a page not there yet plays as silence for
that block, counts a disk underrun (the status bar's "Disk too slow") and is asked for. One disk
thread loads what was asked for, then 4 s ahead of the playhead and the loop start, then fills free
room nearest the playhead; a locate or play start loads its first second before rolling. Offline
work (bounce, freeze, Process, analysis, exports) reads through the same cache but blocks, so it
never has gaps. Sources smaller than `disk::memory_threshold()` (65536 samples by default) stay
decoded in memory; so do the samples of instruments that read them whole (Lift, Juke). Other
formats and rates are decoded once into the project's `Decoded/` folder and stream from there;
generated audio (Process, consolidate, freeze, stems, takes) is written to WAV when it is made.
Waveform overviews are built in one streaming pass and cached in `Waveforms/`. Saves, autosaves
and exports write sources a block at a time.

## Commands and undo

Every user action is a `CommandSpec` (id, label, menu path, shortcut, params doc, enabled, run)
registered in `crates/engine/src/cmd/`. `Engine::execute(id, params)` runs it, catches any escaped
panic, and pushes an undo snapshot (the previous `Arc<Session>`) when the document changed.
Continuous gestures use `execute_merged` so a fader drag is one undo step. Menus are built from
the incumbent's menu catalog (`crates/engine/catalog/menus.txt`); a menu item lights up when a
command has the same menu path and label (or an alias maps it), which also drives the parity
report.

## Audio

`soundcraft_mix::MixEngine::render(session, pos, frames, out)` renders one block: clips (gain,
fades, clip effects) → trim → inserts → pre-fader sends → fader/mute (automation, VCA, trim
automation) → post-fader sends → pan → busses → aux inputs → master faders. Independent strips
process in parallel; plugin delay compensation aligns every path, the master faders' inserts included. The same engine renders
bounces offline (`RangeRender`, streamed to the file block by block) and realtime playback
(`soundcraft_playback::Player`, which owns it on the audio thread and receives new session
snapshots through a channel).

The **monitor stage** (`soundcraft_dsp::monitor`) is the one thing only the listener hears. The
player runs it on the stereo main mix just before the device, after the meters and the remote tap:
a simulated output (SIM: a curve, high/low-pass and stereo width) and then a hearing correction
(EAR: one curve per ear), both minimum-phase biquad cascades, so it adds no latency. `RangeRender`
has no such stage, so bounces, exports, freezes and commits cannot contain it. What is on, and with
which profile, is in `ui.json`, not the session; the profiles are JSON files in the Library folder's
`Monitor Profiles` (`soundcraft_engine::monitor_profiles`). A simulated output is measured, not
drawn: `freedawm-cli monitor-test-file` writes a test signal, and `freedawm-cli monitor-profile`
reads a bounce of it through a device into a profile (`soundcraft_engine::monitor_capture`:
frequency response and stereo width, nothing level-dependent). A hearing profile comes from the Hearing Test
(`soundcraft_dsp::hearing_test`): bands of noise centred between the ears and matched in loudness
against the ISO 226 contour, played as previews, which bypass the stage.

Device I/O today is the system's *default* output and *default* input device, opened as two
separate cpal streams; the mix renders 512-sample blocks and resamples to the device rate when
they differ. There is no device or buffer-size selection, no ASIO, and no MIDI device layer yet
(see [`hardware-parity.md`](hardware-parity.md)). Sources play from disk (above) and each take
records straight to its own file.

`soundcraft-video` holds our own H.264, ProRes and Motion JPEG decoders and the ISO-BMFF demuxer
for the video track. Hosted plugins (`clap-host`, `vst3-host`, `au-host`) are scanned, created and
destroyed off the audio thread; only `process` runs on it.

`soundcraft-vocal`: vocal pitch tracking, notes and formant-preserving pitch/level resynthesis for the Vocal Editor.

The cpal callbacks mark their thread (`soundcraft_playback::mark_audio_thread`). Code that may run
there can ask `on_audio_thread()` before doing anything that blocks; the desktop app's logger does,
so a `log::` record from the audio thread (a stream error, a full synth event queue, a hosted
plugin's failed `process`, a CLAP plugin's own log call) is kept without waiting and written later
by the UI thread (see README › Logs).

## Agent control

The desktop app's control channel (JSON lines over TCP) exposes engine commands, inspection,
synthetic input and screenshots; `freedawm-cli mcp` wraps either a headless engine or a running
app as an MCP server. See `control-protocol.md` and `mcp.md`.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-21 | minor | Sources play from disk through the block cache; the in-memory threshold; recordings go to their files |
| 2026-11-16 | minor | The monitor stage (EAR and SIM): heard on the device only, never rendered |
| 2026-10-13 | trivial | delay compensation includes the master faders' inserts (#351) |
| 2026-10-10 | minor | Added video, vst3-host and au-host to the layer diagram; documented device I/O limits and in-memory media |
| 2026-10-05 | major | First architecture overview |
