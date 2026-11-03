# Hardware parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-30 · **Change:** minor (hardware MIDI input: USB and 5-pin ports, MIDI Thru, monitoring and recording through instruments, Musical Typing; awaiting the owner's pass with a real keyboard and the Focusrite MIDI In; previously trivial: renamed to FreeDAWm, sessions saved as .fdawm; previously minor: audio I/O: mix at the device buffer, duplex feed, I/O Setup channel maps, low-latency monitoring, record offset and loopback measure, ASIO driver latency; awaiting the owner's [hardware checklist](hardware-checklist.md); previously trivial: bounces of a bus fed by an aux compensate its latency (#246)) · **Compared with:** Avid Pro Tools Ultimate 2026.4.1

> Pro Tools, AudioSuite and Avid are trademarks of Avid Technology, Inc. They are named here only to
> compare features (a workflow many users already know); FreeDAWm is not affiliated with or endorsed
> by Avid. See [NOTICE](../NOTICE).

A DAW is judged by how it drives audio interfaces, MIDI gear, control surfaces and sync. This is
FreeDAWm's weakest dimension. Ours is measured from `crates/playback` (cpal) and the absence of
any MIDI-device or control-surface code; Pro Tools' from its documentation and Setup menus.

**Hardware: ~12 % ready** (estimated). **Performance: ~35 %** (estimated; nothing benchmarked
against Pro Tools). Remaining ≈ 110–180 h for hardware, 40–70 h for performance.

Legend: ✅ works · 🟡 partial · ❌ missing.

## Audio interfaces

| Feature | Pro Tools (macOS / Windows) | FreeDAWm macOS | Windows | Linux | FreeBSD | Web |
|---|---|---|---|---|---|---|
| Driver API | Core Audio / ASIO | 🟡 Core Audio via cpal | 🟡 WASAPI shared via cpal; ASIO with the opt-in `asio` build feature (`cargo xtask build-asio`; verified with an SSL 2+, X32/2i2/Solo pass pending) | 🟡 ALSA/Pulse via cpal; no JACK | 🟡 cpal (ALSA layer) | 🟡 Web Audio |
| Choose playback device | ✅ Playback Engine | 🟡 Setup › Playback Engine (not yet verified on macOS) | ✅ Setup › Playback Engine (WASAPI, ASIO) | 🟡 (not yet verified) | 🟡 (not yet verified) | ❌ |
| Choose input device | ✅ | 🟡 Setup › Playback Engine (not yet verified on macOS) | ✅ (WASAPI, ASIO) | 🟡 (not yet verified) | ❌ no recording | ❌ |
| Buffer size (32–2048) | ✅ | 🟡 device buffer when the driver allows it (not yet verified); the mix runs at the device buffer | 🟡 device buffer 16–8192 (verified 128 on ASIO, 256 on WASAPI); the mix runs at the device buffer when the rates match (96/128 pass pending) | 🟡 (not yet verified) | 🟡 | ❌ |
| Duplex stream on one clock | ✅ | 🟡 one-clock input feed for one device (not yet verified) | 🟡 one-clock feed (ASIO, same device); drift-corrected feed for two WASAPI devices (pass pending) | 🟡 (not yet verified) | 🟡 (not yet verified) | ❌ |
| Hardware I/O mapping (I/O Setup to physical channels) | ✅ | 🟡 named inputs and outputs on device channels (not yet verified) | 🟡 named inputs and outputs on device channels; missing ones "(not available)" (X32 pass pending) | 🟡 (not yet verified) | 🟡 (not yet verified) | ❌ |
| Low-latency monitoring | ✅ | 🟡 bypasses latent inserts on live audio tracks (not yet verified) | 🟡 transport LLM key, "LLM" badge (pass pending) | 🟡 | 🟡 | ❌ |
| Latency reporting and record offset compensation | ✅ | 🟡 cpal timestamps, loopback Measure, manual offset (not yet verified) | 🟡 ASIO driver figures or WASAPI timestamps, loopback Measure, manual offset, per interface/buffer/rate (pass pending) | 🟡 | 🟡 | ❌ |
| Sample rate follows the session; resample when it can't | ✅ (switches device) | 🟡 resamples (input pitch bug fixed, #28) | 🟡 | 🟡 | 🟡 | 🟡 |
| Avid HDX / HD Native DSP, Hybrid Engine DSP mode | ✅ | ❌ (out of reach: Avid hardware) | ❌ | ❌ | ❌ | ❌ |
| Disk allocation / record to disk | ✅ | 🟡 takes stream to one WAV each; no disk allocation | 🟡 | 🟡 | 🟡 | ❌ |

## MIDI

| Feature | Pro Tools | FreeDAWm (all platforms) |
|---|---|---|
| MIDI input devices (keyboards, pads) | ✅ | ✅ USB and 5-pin MIDI In ports through `midir` (WinMM, CoreMIDI, ALSA; not on the web): Setup › MIDI Input Devices (Enable, activity light, hot-plug rescan every 2 s); instrument/MIDI track input MIDI: All Inputs or a port, Omni or 1-16; armed or monitored tracks play it through their instrument and record it (punch, loop passes, Input Quantize). Awaiting the owner's pass with real hardware |
| MIDI output to external gear | ✅ | ❌ |
| MIDI Thru, input filter | ✅ | 🟡 MIDI Thru (armed tracks echo, on by default); no input filter |
| MIDI Beat Clock out, MTC generate/chase | ✅ | ❌ |
| MIDI learn / controller mapping | ✅ (via surfaces and plug-in maps) | ❌ |
| Virtual MIDI keyboard | ✅ (MIDI Keyboard window) | ✅ Musical Typing: the MIDI Keyboard window and the computer keys are a live port (Play / Step) |

## Control surfaces

| Feature | Pro Tools | FreeDAWm |
|---|---|---|
| EUCON (S1/S3/S4/S6, Dock, Control app) | ✅ | ❌ Avid-proprietary SDK; owner decision |
| HUI | ✅ | ❌ |
| Mackie Control Universal | ✅ (via HUI emulation and personalities) | ❌ |
| Generic MIDI controller personalities | ✅ | ❌ |

## Sync and video hardware

| Feature | Pro Tools | FreeDAWm |
|---|---|---|
| LTC / MTC chase, 9-pin machine control | ✅ (Ultimate) | ❌ |
| Word clock / video reference awareness | ✅ via interface | ❌ |
| Video output to a second display or video card | ✅ | ❌ (video window only) |
| Satellite linking of systems | ✅ | ❌ |

## Performance

| Feature | Pro Tools | FreeDAWm | Evidence |
|---|---|---|---|
| Parallel mixing across cores | ✅ | ✅ | `soundcraft-mix` renders independent strips in parallel (rayon) |
| Plugin delay compensation | ✅ | ✅ | Bounces of a bus that an aux returns onto are compensated like the main mix (#246). |
| Disk streaming of long sources | ✅ | ❌ | Whole files decoded into the `SourcePool` |
| Track count (Ultimate: 2,048 voices) | ✅ | unmeasured | No benchmark exists; add `cargo xtask bench` |
| Real-time audio thread discipline | ✅ | ✅ | No blocking/allocation in steady state, audio-thread-aware logger |
| Freeze / commit to save CPU | ✅ | 🟡 | `track.commit` renders; `track.freeze` only sets a flag (no CPU saved) |
| GPU-accelerated UI | — | ✅ (wgpu/glow via eframe) | llvmpipe X11 issue #107 |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-30 | minor | Hardware MIDI input (spec 2026-10-30-midi-input): ports, MIDI Thru, track MIDI inputs, live monitoring and recording through instruments, Musical Typing. Readiness estimates unchanged until the owner's pass |
| 2026-10-26 | trivial | The product is renamed FreeDAWm (was SoundCraft); sessions are saved as `.fdawm` and `.scraft` still opens. Estimates unchanged |
| 2026-10-20 | minor | Audio I/O built: mix at the device buffer, duplex feed, I/O Setup channel maps, LLM, record offset and loopback Measure, ASIO driver latency; awaiting the owner's hardware checklist. Record to disk noted. Readiness estimates unchanged until that pass |
| 2026-10-13 | trivial | bounces of a bus fed by an aux compensate its latency (#246) |
| 2026-10-12 | minor | Audio device settings: system, output, input, sample rate and buffer size; opt-in ASIO |
| 2026-10-10 | major | Created: audit of audio interface, MIDI, control-surface, sync and performance support against Pro Tools 2026.4.1 |
