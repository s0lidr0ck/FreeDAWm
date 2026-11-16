# Four-DAW parity, section 3: mixing

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-16 · **Change:** minor (tier 2: Mix faders and pan knobs follow their automation) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

> Pro Tools, Studio Pro / Studio One, Logic Pro and Cubase are trademarks of their owners (Avid,
> Fender/PreSonus, Apple, Steinberg). They are named here only to compare features; FreeDAWm is not
> affiliated with or endorsed by any of them. See [NOTICE](../../NOTICE).

The third of seven sections ([1: playing audio](01-playback-audio.md),
[2: recording audio](02-recording-audio.md)).

**Scope of this section:** the mixer: channel types and the strip, panning, inserts and sidechains
as the mixer hosts them, sends and routing, groups and solo, automation, metering, mixdown and
export, surround, and how mixer state is managed. The effects themselves are section 4. Control
surfaces are named here where automation depends on them and belong to the hardware work
([`hardware-parity.md`](../hardware-parity.md)).

## How this was measured

- **FreeDAWm:** read from the code on the working branch (0.9.11 plus sections 1 and 2), not from
  the docs. Options that store a value nothing reads are marked **flag only**. The headline claims
  were re-read by hand. Nothing was listened to or measured.
- **The four DAWs:** from their published manuals and release notes, as known up to the versions
  above, for each product's top edition. Nothing was run side by side.

**Legend.** Other DAWs: ● has it · ◐ partly or by another route · ○ does not.
FreeDAWm: **Works** · **Partial** · **Flag only** · **Missing**.

## Summary

| Area | FreeDAWm today | Distance from the four |
|---|---|---|
| A. Channels and the strip | Every channel type, formats to 9.1.6, +12 dB faders | Small |
| B. Panning | Four pan laws, dual pan for stereo | Medium: no balance or width |
| C. Inserts and sidechains | 32 modules, drag to reorder and copy, chain presets | **Large**: no key input on the stock compressor or on any third-party plug-in |
| D. Sends and routing | 10 sends, busses of any format, routing folders | Medium: sends reach busses only |
| E. Groups, VCAs, solo | Path-aware solo in place, solo safe, relative groups with a choice of attributes | Medium: no listen bus, one VCA level |
| F. Automation | All modes write; trim lane; wide editing set; fader section sample-accurate and delay-compensated; faders follow | **Large**: plug-in values block-rate, no controller input |
| G. Metering | Peak with hold and clip lights, no peak missed, pre- or post-fader | **Large**: one scale, no RMS |
| H. Mixdown and export | Offline bounce to five formats, stems, DAWproject | Medium: main mix only in the dialog, no tail, no metadata |
| I. Surround | Beds to 9.1.6, a surround panner, fold-down | **Large**: all four now render Atmos objects and binaural |
| J. Mixer state and window | Undo for every move, track and chain presets | Small to medium: no snapshots, no per-track delay |

Three things every one of the four does and FreeDAWm does not: **plug-in automation that is
sample-accurate and delay-compensated** (the fader section's now is, and the faders follow it); **a key input on the stock
compressor and on third-party plug-ins**; and **meters with more than one scale** (meters no longer
miss a peak, and can read pre-fader).

## A. Channels and the strip

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Audio, aux, instrument, MIDI, master, VCA, folder channels | ● | ● | ● | ● | **Works** |
| Folder that only organises (no audio path) | ● | ● | ● | ● | **Missing**: every folder is a routing folder |
| Formats from mono to 7.1.4 and wider | ● | ● | ● | ● | **Works** up to 16 channels; Ambisonics orders 4–7 are offered and silently truncated |
| Change a track's format later | ◐ | ● | ● | ◐ | **Works** |
| Input trim and polarity on the strip | ● | ● | ● | ● | **Partial**: trim works; polarity is one switch for all channels and has no button |
| Fader to +12 dB | ● | ◐ | ◐ | ◐ | **Works** |
| Per-track delay in samples or ms | ● | ● | ● | ● | **Missing** for audio tracks |
| Built-in channel processing (EQ, dynamics or console sound in the strip) | ◐ | ◐ | ○ | ● | **Missing**: everything is an insert |
| Make a track inactive to free its plug-ins | ● | ● | ● | ● | **Partial**: rendering stops, the plug-ins stay loaded |

## B. Panning

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Choice of pan law | ● | ● | ● | ● | **Works**: 0, −3, −4.5 or −6 dB, per session (Options › More › Pan Law, `mix.pan_law`) |
| Dual pan for stereo tracks | ● | ● | ● | ● | **Works** |
| Balance, and width or combined panner | ○ | ● | ● | ● | **Missing** |
| Link or mirror left and right pans | ● | ◐ | ◐ | ● | **Missing** |
| Surround panner | ● | ● | ● | ● | **Partial**: position, divergence and height; centre % and LFE have no control; mono and stereo tracks only |
| Binaural pan | ● | ● | ● | ● | **Missing** |

## C. Inserts and sidechains

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Ten or more inserts per channel | ● | ● | ● | ● | **Works**: up to 32 modules |
| Reorder, copy and move by dragging | ● | ● | ● | ● | **Works** |
| Bypass | ● | ● | ● | ● | **Works** |
| Deactivate a plug-in to free CPU | ● | ● | ◐ | ● | **Works**: Inactive in an insert's menu (`mix.module_active`); the instance stays loaded |
| Post-fader inserts | ◐ | ◐ | ○ | ● | **Missing**; the master's inserts run before its fader, where Pro Tools runs them after |
| Chain presets | ● | ● | ● | ● | **Works** |
| Compare (A/B) in every plug-in window | ● | ● | ● | ● | **Partial**: in the built-in editors, not for hosted plug-ins |
| Key input on the stock compressor, gate and de-esser | ● | ● | ● | ● | **Missing**: only Lull, Rez and Shale take a key |
| Key input on third-party plug-ins | ● | ● | ● | ● | **Missing** for CLAP, VST3 and Audio Units |
| Key aligned by delay compensation | ● | ● | ● | ● | **Works** |
| Multi-mono or dual-mono processing | ● | ◐ | ● | ◐ | **Missing** |
| Hardware inserts with latency measurement | ● | ● | ● | ● | **Missing** |

## D. Sends and routing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Eight or more sends, pre or post fader | ● | ● | ● | ● | **Works**: ten |
| Send level, pan and mute, automatable | ● | ● | ● | ● | **Works**: a stereo send's pan balances it |
| Send to a bus, a track or a hardware output | ● | ● | ● | ● | **Partial**: busses only |
| A new send comes with its return | ● | ● | ● | ● | **Works**: a send to a new bus from the Mix window makes its aux (`mix.send` `return: true`) |
| Default send level setting | ● | ◐ | ● | ◐ | **Works**: silent or 0 dB (Options › More › Default Send Level; any level by `mix.default_send_level`) |
| Pre-fader sends that ignore mute, as an option | ● | ◐ | ● | ● | **Missing**: mute silences every send |
| Copy the fader into a send (for cue mixes) | ● | ● | ◐ | ● | **Partial**: copies volume only and ignores the selected range |
| Busses of any format, output to bus or hardware | ● | ● | ● | ● | **Works** |
| Several outputs from one track | ● | ○ | ○ | ◐ | **Missing** |
| Refuse or warn about a feedback loop | ● | ● | ● | ● | **Works**: refused, for outputs, inputs and sends |
| Routing folders, nested | ● | ● | ● | ● | **Works** |

## E. Groups, VCAs and solo

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Groups that link volume, mute, solo and arm | ● | ● | ● | ● | **Works**, relative offsets kept |
| Choose what a group links (pan, sends, inserts, automation mode) | ● | ◐ | ● | ● | **Partial**: volume, mute, solo, record, pan, sends and automation mode, by command (`track.group`, `track.group_modify` `attributes`); the Group dialog does not offer them yet; inserts are not linked |
| Suspend all groups | ● | ● | ● | ● | **Works**: Options › More › Suspend All Groups |
| VCA masters | ● | ● | ● | ● | **Partial**: one level; no nesting; assigned by command only, no control in the mixer; its solo solos its members |
| Solo in place that follows the signal path | ● | ● | ● | ● | **Works**: effect returns of a soloed source are heard |
| Solo safe | ● | ● | ● | ● | **Works** |
| Exclusive solo | ● | ● | ● | ● | **Works** |
| AFL / PFL on a listen bus | ● | ● | ○ | ● | **Flag only** |
| Momentary solo | ● | ○ | ○ | ○ | **Missing**: the menu entry behaves as latch |

## F. Automation

### What can be automated

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Volume, pan, mute, send level, pan and mute | ● | ● | ● | ● | **Works** |
| Plug-in parameters, stock and third-party | ● | ● | ● | ● | **Works** for inserts |
| Instrument parameters | ● | ● | ● | ● | **Missing** |
| Plug-in bypass | ● | ● | ● | ● | **Missing** |
| Surround pan | ● | ● | ● | ● | **Missing** |
| VCA and master fader | ● | ● | ● | ● | **Works** |

### Writing and playing it

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Read, Touch, Latch, Write | ● | ● | ● | ● | **Works**, one undo step per pass |
| Trim automation | ● | ○ | ● | ● | **Works** for volume |
| Faders, knobs and plug-in controls move with automation | ● | ● | ● | ● | **Partial**: the Mix window's faders and pan knobs follow their lanes; plug-in controls and send knobs do not |
| You hear the control you are moving | ● | ● | ● | ● | **Partial**: in Touch and Latch you hear what was written 20 ms earlier |
| Sample-accurate, or at least block-independent | ● | ● | ● | ● | **Partial**: volume, trim, VCA and mute are read sample by sample where a lane bends or breaks inside a block, sends and pan on a 64-sample timeline grid, so playback and bounce match; plug-in values still change once per block |
| Automation delay-compensated | ● | ● | ● | ● | **Partial**: fader, mute, pan and send automation play where their audio is (behind by the strip's plug-in delay); moves are still written early by the output latency |
| Glide back to the old curve on release (return time) | ● | ● | ● | ● | **Missing** |
| Write to start, end or the whole selection on stop | ● | ○ | ○ | ● | **Missing** |
| Preview a value before committing it | ● | ○ | ○ | ● | **Missing** |
| Enable or suspend by parameter type | ● | ◐ | ◐ | ● | **Flag only**: the Automation window's switches are read by nothing |
| Write from a MIDI controller or control surface | ● | ● | ● | ● | **Missing**: no controller mapping, no HUI or Mackie Control |
| Automation follows tempo changes | ● | ● | ● | ● | **Missing**: points are in samples |

### Editing it

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Add, move, delete, nudge points | ● | ● | ● | ● | **Works** |
| Curved segments | ○ | ● | ● | ● | **Works**: one bend per segment |
| Pencil shapes (line, triangle, square, random) | ● | ● | ◐ | ● | **Partial**: freehand and line |
| Cut, copy, paste, paste to another parameter | ● | ● | ● | ● | **Works**: curves and the range's edges come along; a paste replaces what was there |
| Thin, and thin automatically after a pass | ● | ● | ◐ | ● | **Works**: a pass's lanes are thinned when it ends (Options › More › Thin Automation After Pass) |
| Automation moves with clips | ● | ● | ● | ● | **Works**; the curve changes shape at the clip edges |
| Automation inside a clip or region | ○ | ◐ | ● | ◐ | **Missing** |
| Several lanes per track, any parameter | ● | ● | ● | ● | **Works** |
| Show the parameter last touched | ◐ | ● | ● | ● | **Missing** |
| Convert between clip gain and volume automation | ● | ○ | ○ | ○ | **Works** |

## G. Metering

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Peak meters that catch every peak | ● | ● | ● | ● | **Works**: each screen update shows the loudest block since the last one |
| Peak hold and clip lights | ● | ● | ● | ● | **Works**; one bar per channel on surround tracks |
| Pre-fader metering | ● | ● | ● | ● | **Works**: Options › More › Pre-Fader Metering; track strips only (the master meter stays post-fader) |
| RMS, and a choice of scales (VU, K-system, PPM) | ● | ● | ◐ | ● | **Missing**: one scale that stops at 0 dBFS |
| Gain reduction on the strip | ● | ● | ● | ● | **Works**: a number for the first dynamics module |
| Loudness (LUFS) and true peak while mixing | ◐ | ● | ● | ● | **Partial**: only inside the LevelUp module, or as two numbers after a bounce |
| Correlation, spectrum, vectorscope | ● | ● | ● | ● | **Partial**: inside the Sprawl and CleanQ modules and Scout; no meter module of its own |
| Output meter free of the click | ● | ● | ● | ● | **Works** |
| Meters while stopped | ● | ● | ● | ● | **Partial**: only for armed or monitored tracks |

## H. Mixdown and export

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Offline bounce, with progress and cancel | ● | ● | ● | ● | **Works**, through the live plug-in instances |
| Real-time bounce (for outboard gear) | ● | ● | ● | ● | **Missing** |
| Choose the source: any bus or output | ● | ● | ● | ● | **Partial**: by command; the dialog always bounces the main mix |
| Range: selection, loop, markers, whole session | ● | ● | ● | ● | **Works**, except the selection |
| Keep the reverb tail | ◐ | ● | ● | ● | **Missing**: a whole-session bounce ends at the last clip |
| WAV, AIFF, FLAC, MP3 and a modern codec | ● | ● | ● | ● | **Works**: WAV, AIFF, FLAC, MP3, Opus |
| Convert sample rate on export | ● | ● | ● | ● | **Missing** for WAV, AIFF and FLAC |
| Dither with a choice of type and noise shaping | ● | ● | ● | ● | **Partial**: TPDF, always on, no control in the dialog |
| Mono, and one file per channel | ● | ● | ● | ● | **Missing** |
| Normalize on export | ○ | ◐ | ● | ◐ | **Partial**: to sample peak, by rendering twice |
| Tags and broadcast metadata in the file | ● | ● | ● | ● | **Missing** |
| Add the bounce back to the session | ● | ● | ● | ● | **Missing** |
| Several formats in one pass | ● | ◐ | ● | ● | **Missing** |
| Stems in one pass, with a choice of tracks or busses | ● | ● | ● | ● | **Partial**: every unmuted track, always through the master, behind a progress window with Cancel |
| Commit, freeze, bounce in place | ● | ● | ● | ● | **Works**: commit keeps the format, the pan and the fader automation |
| AAF or OMF | ● | ● | ● | ● | **Missing** |
| DAWproject | ○ | ● | ○ | ● | **Works**, both ways |

## I. Surround and immersive

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Surround tracks, busses and bounce | ● | ● | ● | ● | **Works** up to 16 channels, correct channel order |
| Fold-down to stereo | ● | ● | ● | ● | **Works**: fixed ITU gains |
| Dolby Atmos objects with a built-in renderer | ● | ● | ● | ● | **Flag only**: the object switch is a label |
| Binaural monitoring | ● | ● | ● | ● | **Missing** |
| ADM BWF export | ● | ● | ● | ● | **Missing** |
| Surround pan automation | ● | ● | ● | ● | **Missing** |
| Ambisonics | ● | ○ | ○ | ● | **Partial**: first order only |
| Correct loudness for 7.1 and wider | ● | ● | ● | ● | **Works**: every layout weighted by its speakers |

Immersive delivery is gap G-12 in [`gaps.md`](../gaps.md). Since Studio One 6.5 all four ship an
Atmos renderer, so this has moved from a post-production extra to common ground.

## J. Mixer state and window

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Undo for mixer moves | ◐ | ● | ● | ● | **Works**: every move, one step per gesture |
| Track presets | ● | ● | ● | ● | **Works** |
| Snapshots or scenes of the whole mix | ○ | ● | ◐ | ● | **Missing** |
| Copy one channel's settings onto others | ● | ● | ● | ● | **Partial**: duplicate a track or apply a preset |
| Import mixer settings from another session onto existing tracks | ● | ◐ | ● | ● | **Partial**: imports whole tracks; groups, VCA and folder assignments are dropped |
| Narrow strips, show and hide sections | ● | ● | ● | ● | **Works** |
| Module graphs and gain reduction on the strip | ◐ | ● | ● | ● | **Works**: live graphs on every built-in module tile |
| Find and filter tracks in the mixer | ● | ● | ● | ● | **Missing** |
| Spill a folder, VCA or bus | ● | ◐ | ○ | ◐ | **Missing** |
| Several mixer windows | ○ | ● | ● | ● | **Partial**: one, detachable |

## Bugs found on the way

Lines move while the earlier sections' work is in progress, so functions are named.

| # | Bug | Where |
|---|---|---|
| 1 | **Fixed:** meters and clip lights showed only the latest block. The engine and the player now keep the loudest block until the UI takes it (`StripMeter::merge_from`, `Player::copy_meters`), and a block the audio thread could not publish is folded into the next | `StripMeter::measure` in `crates/mix/src/lib.rs`; the meter copy in `crates/playback/src/lib.rs` |
| 2 | **Fixed:** on a stereo track the send pan did nothing; it now balances the send (turned left, the right side moves left until both are hard left) | `route_strip` in `crates/mix/src/lib.rs` |
| 3 | **Fixed:** soloing a VCA muted everything; a soloed VCA master now solos its members and what their paths reach (`SoloScope::fill`, which the engine and the S keys share) | `SoloScope::fill` in `crates/model/src/solo.rs`; the S key in `mix_window.rs` |
| 4 | **Fixed:** the trim lane played with automation Off; it now reads like every other lane | fader stage of `process_strip` in `crates/mix/src/lib.rs` |
| 5 | **Fixed:** a mute switched in one sample and could click; it now ramps over 5 ms (240 samples), from where the last block left it | the same place |
| 6 | **Fixed:** group members stopped writing half a second after the dragged fader stood still; holding a grouped fader or mute now holds the group's (`with_group` in `auto_write.rs`) | `mix.volume` grouping in `crates/engine/src/cmd/mix.rs`; `auto_write.rs` |
| 7 | **Fixed:** paste and duplicate dropped curves and left the old points under the new; a copy now carries its curves and its edges (`AutomationLane::slice`), and a paste replaces what was under it with anchors either side (`paste_slice`); Paste Automation Points replaces the points between the first and the last. A copy that starts or ends inside a bent segment keeps that segment's bend, which only approximates its cut-off part | `edit.paste`, `edit.duplicate`, `automation.paste` |
| 8 | **Fixed:** Commit put a surround track on a stereo track and dropped pan and automation; the committed track now has the source's format, pan, surround panner, mute, VCA and fader, pan, mute and send automation (the trim and the modules' lanes are in the audio) | `commit_track` in `crates/engine/src/io.rs` |
| 9 | **Fixed:** loudness weighted any 6-channel bounce as 5.1 and every wider one flat; each channel is now weighted by its speaker as BS.1770-4 says (`ChannelFormat::loudness_weights`: 0 for the LFE, 1.41 for speakers between 60° and 120°) | `LoudnessMeter::new` in `crates/dsp/src/meter.rs` |
| 10 | **Fixed:** Export › Stems ran in the window's thread with no progress or cancel; from the menu it now stops the transport and renders behind a progress window a slice per frame, with Cancel (`StemsJob`, `bounce::start_stems`); agents and the CLI still run it to the end | the stems prompt in `crates/ui-egui/src/dialogs.rs` |
| 11 | **Fixed:** the bounce dialog offered FLAC at 32-bit float and beyond 8 channels; it now offers 16 and 24 bit for FLAC and refuses a FLAC bounce of a mix wider than 8 channels before rendering | `dialogs.rs`; `crates/audio-io/src/flac.rs` |
| 12 | **Fixed:** the output meter measured the click with the mix; it now measures the mix before the click and the count-off are added | master stage of `MixEngine::render` |
| 13 | **Fixed:** loading a track preset kept the old modules' automation, which could drive the new; their lanes now go with them (the fader's, pan's and sends' stay) | `track.load_preset` in `crates/engine/src/cmd/track_more.rs` |
| 14 | **Not a bug (the claim was wrong):** a mono track feeding a mono bus already takes its pan law there (−3 dB), so the stereo aux spreading that bus at unity plays it exactly as loud as the track centred to the main mix; a test now pins it | bus read in `process_strip` |
| 15 | **Fixed:** an output, send or input that fed a track back into itself was accepted and went silent; `track.output`, `track.input` and `mix.send` now refuse it (`Session::route_loops`, `input_loops`) | `track.output`, `mix.send` |
| 16 | **Fixed:** Audio Units were not told a render is offline; the unit now gets `kAudioUnitProperty_OfflineRender` before it is initialized for a bounce, freeze or commit (macOS only, not run on hardware) | `crates/au-host` |

## Suggested order of work

Hours are agent-hours on this repo's calibration ([`target-app-parity.md`](../target-app-parity.md)); all are judgements.

**1. Fix what is wrong (≈ 14–22 h).** Bugs 1–16. Bug 1 first: a meter that can miss a clip is the
one a mix engineer would not forgive.

**2. What all four have (≈ 110–170 h).**

| Work | Est. (h) | Notes |
|---|---:|---|
| Automation playback: breakpoints honoured inside a block, sends, mute and plug-in values ramped, all of it delay-compensated, identical in a bounce | 14–22 | Closes "interior breakpoints" in `gaps.md` G-04. **Done for the fader section** (volume, trim, VCA, mute, pan, sends); plug-in values remain once per block, not delay-compensated |
| Automation writing: controls follow automation, you hear what you move, written at the heard position, return time on release, thin after a pass | 10–16 | **Thin after a pass done; faders and pan knobs follow**; the rest remains |
| Automate plug-in bypass, instrument parameters and surround pan; make the per-type enables real | 6–10 | |
| Key inputs: on the stock compressor, gate and de-esser, and sidechain busses for CLAP, VST3 and AU | 12–20 | Part of G-13 |
| Controller mapping (MIDI learn) for faders and plug-in parameters | 8–12 | Needs MIDI input, which exists; surfaces (HUI/MCU) stay in G-10 |
| Metering: pre-fader option, RMS and scale choice, a loudness and true-peak meter on the master, output meter without the click | 8–12 | **Pre-fader option and click-free output meter done**; RMS, scales and a master loudness meter remain |
| Pan law choice; balance and width for stereo; per-track delay | 6–10 | **Pan law done**; balance, width and per-track delay remain |
| Sends to tracks and hardware outputs, a return made with a new send, default send level, loop refusal | 6–10 | Shared with section 2's cue mixes. **Return, default level and loop refusal done**; sends to tracks and hardware outputs remain |
| Bounce dialog: source, tail, sample rate, dither and mono options, metadata, add to session; stems as a background job with a choice of tracks and busses | 12–18 | |
| Real-time bounce | 4–6 | |
| Group attributes, suspend all, VCA assignment in the mixer, nested VCAs | 6–10 | **Attributes (by command) and suspend all done**; attributes in the Group dialog, VCA assignment in the mixer and nested VCAs remain |
| Atmos: objects, a renderer of our own, binaural monitoring, ADM BWF | 50–80 | Is G-12; listed here because all four now have it, sized there |

**3. What most have (≈ 40–65 h).** Mixer snapshots (Studio Pro, Cubase), 8–12; a listen bus for
AFL and PFL (all but Logic), 6–10; post-fader inserts and plug-in deactivation (**deactivation done**), 5–8; hardware
inserts with a latency ping, 6–10, hardware needed; multi-mono processing, 6–10; write-on-stop and
preview (Pro Tools, Cubase), 5–8; folders without an audio path, mixer search and spill, 5–8.

**Where FreeDAWm is already level or ahead:** solo that follows the signal path, the breadth of
delay compensation including sidechain keys, undo for every mixer move, freeze up to a chosen
module, live graphs on module tiles, curved automation segments (Pro Tools has none), conversion
between clip gain and automation, and DAWproject in both directions.

## Sources

Versions as in [section 1](01-playback-audio.md#sources). Feature entries for the four come from
their manuals as known, not from a side-by-side run; treat a ◐ as "check before relying on it".

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-16 | minor | tier 2: Mix faders and pan knobs follow their automation |
| 2026-11-16 | minor | tier 2: automation is thinned after a writing pass |
| 2026-11-16 | minor | tier 3: a module can be made inactive |
| 2026-11-16 | minor | tier 2: a new send brings its return and takes a default level |
| 2026-11-16 | minor | tier 2: group attributes and Suspend All Groups |
| 2026-11-16 | minor | tier 2: fader-section automation sample-accurate and delay-compensated |
| 2026-11-16 | minor | tier 2: pre-fader metering |
| 2026-11-16 | minor | tier 2: a choice of pan law |
| 2026-11-16 | minor | bug 16 fixed: Audio Units are told a render is offline |
| 2026-11-16 | minor | bug 15 fixed: routing loops are refused |
| 2026-11-16 | minor | bug 14 was not one: a mono bus into a stereo aux is as loud as a centred mono track |
| 2026-11-16 | minor | bug 13 fixed: a track preset drops the old modules' automation |
| 2026-11-16 | minor | bug 12 fixed: the output meter leaves the click out |
| 2026-11-16 | minor | bug 11 fixed: the bounce dialog offers only what FLAC can write |
| 2026-11-16 | minor | bug 10 fixed: stems export behind a progress window with Cancel |
| 2026-11-16 | minor | bug 9 fixed: loudness is weighted by speaker for every layout |
| 2026-11-16 | minor | bug 8 fixed: commit keeps the format, pan and automation |
| 2026-11-16 | minor | bug 7 fixed: paste and duplicate keep curves and replace the old points |
| 2026-11-16 | minor | bug 6 fixed: a held grouped fader holds its group |
| 2026-11-16 | minor | bug 5 fixed: mutes ramp instead of clicking |
| 2026-11-16 | minor | bug 4 fixed: the trim lane is silent with automation Off |
| 2026-11-16 | minor | bug 3 fixed: soloing a VCA solos its members |
| 2026-11-16 | minor | bug 2 fixed: a stereo send follows its pan |
| 2026-11-16 | minor | bug 1 fixed: meters keep every peak between screen updates |
| 2026-11-16 | major | First version: mixing against Pro Tools, Studio Pro, Logic Pro and Cubase |
