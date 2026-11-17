# Four-DAW parity, section 3: mixing

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-17 · **Change:** minor (rows reconciled with the code) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

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
| B. Panning | Four pan laws, dual pan, width and balance for stereo | Small: no binaural or linked pans |
| C. Inserts and sidechains | 32 modules, drag to reorder and copy, chain presets, CLAP key inputs | Medium: no key on VST3 or AU |
| D. Sends and routing | 10 sends to busses, tracks or outputs, busses of any format, routing folders | Small: one output per track |
| E. Groups, VCAs, solo | Path-aware solo in place, AFL/PFL on a listen bus, solo safe, relative groups with a choice of attributes, nested VCAs | Small: no momentary solo |
| F. Automation | All modes write; trim lane; wide editing set; fader section sample-accurate and delay-compensated; plug-in values delay-compensated; faders follow | **Large**: plug-in values block-rate |
| G. Metering | Peak with hold and clip lights, no peak missed, pre- or post-fader, RMS/VU/K scales, a master loudness meter | Small: no PPM, no loudness range |
| H. Mixdown and export | Offline bounce of the mix or a bus to five formats, with a tail; stems; DAWproject | Medium: no metadata, no sample-rate conversion |
| I. Surround | Beds to 9.1.6, a surround panner, fold-down | **Large**: all four now render Atmos objects and binaural |
| J. Mixer state and window | Undo for every move, track and chain presets, per-track delay, snapshots | Small: no mixer search or spill |

Two things every one of the four does and FreeDAWm does not yet: **plug-in automation that is
sample-accurate** (the fader section's now is, and the faders follow it; plug-in values are delay-compensated but block-rate); and **a key input on
VST3 and AU plug-ins** (CLAP plug-ins and the stock compressor and gates take one). Meters now have RMS, VU and
K-system scales, and the master a loudness meter.

## A. Channels and the strip

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Audio, aux, instrument, MIDI, master, VCA, folder channels | ● | ● | ● | ● | **Works** |
| Folder that only organises (no audio path) | ● | ● | ● | ● | **Missing**: every folder is a routing folder |
| Formats from mono to 7.1.4 and wider | ● | ● | ● | ● | **Works** up to 16 channels; Ambisonics orders 4–7 are offered and silently truncated |
| Change a track's format later | ◐ | ● | ● | ◐ | **Works** |
| Input trim and polarity on the strip | ● | ● | ● | ● | **Works**: trim, and a Ø button beside it on the strip's preamp row: a click inverts every channel, a right-click single ones (`mix.phase_invert` `channel`) |
| Fader to +12 dB | ● | ◐ | ◐ | ◐ | **Works** |
| Per-track delay in samples or ms | ● | ● | ● | ● | **Works**: Track › Strip › Delay (`mix.track_delay`, samples or ms, up to 192 000 samples), before the inserts, left in by delay compensation |
| Built-in channel processing (EQ, dynamics or console sound in the strip) | ◐ | ◐ | ○ | ● | **Missing**: everything is an insert |
| Make a track inactive to free its plug-ins | ● | ● | ● | ● | **Partial**: rendering stops, the plug-ins stay loaded |

## B. Panning

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Choice of pan law | ● | ● | ● | ● | **Works**: 0, −3, −4.5 or −6 dB, per session (Options › More › Pan Law, `mix.pan_law`) |
| Dual pan for stereo tracks | ● | ● | ● | ● | **Works** |
| Balance, and width or combined panner | ○ | ● | ● | ● | **Works** for stereo tracks: Track › Strip › Width and Balance (`mix.width`, `mix.balance`) on the main output, around the dual pans; automatable (width and balance lanes, on the pan's 64-sample grid; moving the controls counts as a touch for writing, not yet tried in a writing pass) |
| Link or mirror left and right pans | ● | ◐ | ◐ | ● | **Missing** |
| Surround panner | ● | ● | ● | ● | **Partial**: position, divergence and height; centre % and LFE have no control; mono and stereo tracks only |
| Binaural pan | ● | ● | ● | ● | **Not planned here**: needs a measured head-related impulse response set, content the owner sources |

## C. Inserts and sidechains

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Ten or more inserts per channel | ● | ● | ● | ● | **Works**: up to 32 modules |
| Reorder, copy and move by dragging | ● | ● | ● | ● | **Works** |
| Bypass | ● | ● | ● | ● | **Works** |
| Deactivate a plug-in to free CPU | ● | ● | ◐ | ● | **Works**: Inactive in an insert's menu (`mix.module_active`); the instance stays loaded |
| Post-fader inserts | ◐ | ◐ | ○ | ● | **Works**: Post-Fader in an insert's menu (`mix.module_post_fader`): the module runs after the fader and mute, on any track and on the master (after the master fader, where Pro Tools runs the master's inserts); it is not in the pre-fader copy (pre-fader sends, keys, meters), delay compensation counts it and holds the pre-fader sends back for it, a freeze leaves it live and a commit moves it, with its lanes, onto the new track |
| Chain presets | ● | ● | ● | ● | **Works** |
| Compare (A/B) in every plug-in window | ● | ● | ● | ● | **Partial**: in the built-in editors, not for hosted plug-ins |
| Key input on the stock compressor, gate and de-esser | ● | ● | ● | ● | **Works**: Comp (every style, its detector measures the key), the gates Lull and Shale, and Rez take a key; the routing side is generic (any plugin whose `PluginInfo::sidechain` is set gets the Sidechain row in its insert header and a delay-compensated key from any track); any track can be keyed from an aux or bus return (it renders after that aux; a key from an aux the track feeds is refused as a loop) |
| Key input on third-party plug-ins | ● | ● | ● | ● | **Partial**: CLAP plug-ins with a second input port are keyed (from the insert header, delay-compensated); VST3 aux buses and Audio Units are not |
| Key aligned by delay compensation | ● | ● | ● | ● | **Works** |
| Multi-mono or dual-mono processing | ● | ◐ | ● | ◐ | **Missing** |
| Hardware inserts with latency measurement | ● | ● | ● | ● | **Missing** |

## D. Sends and routing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Eight or more sends, pre or post fader | ● | ● | ● | ● | **Works**: ten |
| Send level, pan and mute, automatable | ● | ● | ● | ● | **Works**: a stereo send's pan balances it; Pan Follows Main in the send's menu makes it take the track's pan and its automation |
| Send to a bus, a track or a hardware output | ● | ● | ● | ● | **Works**: a send's menu offers busses, Output ▸ (straight to a hardware output) and Track ▸ (the track's input becomes the bus the send feeds) (`mix.send` `output`, `to_track`) |
| A new send comes with its return | ● | ● | ● | ● | **Works**: a send to a new bus from the Mix window makes its aux (`mix.send` `return: true`) |
| Default send level setting | ● | ◐ | ● | ◐ | **Works**: silent or 0 dB (Options › More › Default Send Level; any level by `mix.default_send_level`) |
| Pre-fader sends that ignore mute, as an option | ● | ◐ | ● | ● | **Works**: Options › More › Pre-Fader Sends Ignore Mute (`mix.pre_sends_ignore_mute`), also through a solo's mute |
| Copy the fader into a send (for cue mixes) | ● | ● | ◐ | ● | **Works**: Edit › Automation › Copy to Send copies the volume, and pan and mute when asked (`automation.copy_to_send` `what`), over the edit selection when there is one (what the send had there is replaced, its curve either side kept), else the whole lane; without automation the values themselves; the Control Room's Add Cue Sends makes a pre-fader send to a cue on every track at its fader level (`monitor.cue_sends`) |
| Busses of any format, output to bus or hardware | ● | ● | ● | ● | **Works** |
| Several outputs from one track | ● | ○ | ○ | ◐ | **Missing** |
| Refuse or warn about a feedback loop | ● | ● | ● | ● | **Works**: refused, for outputs, inputs and sends |
| Routing folders, nested | ● | ● | ● | ● | **Works** |

## E. Groups, VCAs and solo

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Groups that link volume, mute, solo and arm | ● | ● | ● | ● | **Works**, relative offsets kept |
| Choose what a group links (pan, sends, inserts, automation mode) | ● | ◐ | ● | ● | **Works** for volume, mute, solo, record, pan, sends and automation mode, in the Group dialog (Create and Modify) and by command; inserts are not linked |
| Suspend all groups | ● | ● | ● | ● | **Works**: Options › More › Suspend All Groups |
| VCA masters | ● | ● | ● | ● | **Works**: assigned from the track menu (VCA ▸, in the Edit and Mix windows), nested up to eight deep (a VCA follows another; loops refused), each one's fader, mute and solo reaching the members below |
| Solo in place that follows the signal path | ● | ● | ● | ● | **Works**: effect returns of a soloed source are heard |
| Solo safe | ● | ● | ● | ● | **Works** |
| Exclusive solo | ● | ● | ● | ● | **Works** |
| AFL / PFL on a listen bus | ● | ● | ○ | ● | **Works**: in the AFL and PFL solo modes (Options › Solo Mode, or the Control Room) a solo mutes nothing; the soloed tracks are summed after the fader and pan (AFL) or before the fader (PFL) into a listen bus that replaces the mix on the monitor path while any is soloed, at its own level; the mix, bounces, stems and cue mixes are untouched (stems ignore an AFL or PFL solo, the owner's decision: each stem is its own track as with no solo) |
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
| You hear the control you are moving | ● | ● | ● | ● | **Partial**: in Touch and Latch you hear what was written up to 20 ms earlier, now at the position you heard it |
| Sample-accurate, or at least block-independent | ● | ● | ● | ● | **Partial**: volume, trim, VCA and mute are read sample by sample where a lane bends or breaks inside a block, sends and pan on a 64-sample timeline grid, so playback and bounce match; plug-in values still change once per block |
| Automation delay-compensated | ● | ● | ● | ● | **Works**: fader, mute, pan and send automation play where their audio is (behind by the strip's plug-in delay), and a module's parameters where its input is (behind by the modules before it), in playback and bounce alike; moves are written where they were heard (the mix delay and output latency behind the transport) |
| Glide back to the old curve on release (return time) | ● | ● | ● | ● | **Partial**: Touch glides back over 250 ms; the time is fixed |
| Write to start, end or the whole selection on stop | ● | ○ | ○ | ● | **Missing** |
| Preview a value before committing it | ● | ○ | ○ | ● | **Missing** |
| Enable or suspend by parameter type | ● | ◐ | ◐ | ● | **Works**: the Automation window's switches (`automation.enable`) stop a type being written; its lanes still play |
| Write from a MIDI controller or control surface | ● | ● | ● | ● | **Works**: a Mackie Control or HUI surface's touch-sensitive faders write as the on-screen fader held does (Touch, Latch), its V-Pots write pan (section 6, K); MIDI learn maps a controller to a fader, pan, send level or plug-in knob, written as the on-screen control moved |
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
| RMS, and a choice of scales (VU, K-system, PPM) | ● | ● | ◐ | ● | **Works**: Preferences › Metering › Scale: peak, RMS (300 ms), peak with RMS inside, VU (0 VU = −18 dBFS), K-20, K-14, K-12, numbered from their reference; release and peak hold beside it; saved with this computer's settings. No PPM ballistics |
| Gain reduction on the strip | ● | ● | ● | ● | **Works**: a number for the first dynamics module |
| Loudness (LUFS) and true peak while mixing | ◐ | ● | ● | ● | **Works**: Window › Loudness Meter reads the master as it plays: momentary, short-term, integrated, maximum momentary and true peak, with Reset; no loudness range |
| Correlation, spectrum, vectorscope | ● | ● | ● | ● | **Works**: the Spectrum and Phase Meter plug-ins (Utilities in the insert menu), as well as inside the Sprawl and CleanQ modules and Scout |
| Output meter free of the click | ● | ● | ● | ● | **Works** |
| Meters while stopped | ● | ● | ● | ● | **Partial**: only for armed or monitored tracks |

## H. Mixdown and export

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Offline bounce, with progress and cancel | ● | ● | ● | ● | **Works**, through the live plug-in instances |
| Real-time bounce (for outboard gear) | ● | ● | ● | ● | **Missing** |
| Choose the source: any bus or output | ● | ● | ● | ● | **Works** for the main mix and busses, in the dialog; hardware outputs are not offered |
| Range: selection, loop, markers, whole session | ● | ● | ● | ● | **Works**: the selection is the loop range (one thing in FreeDAWm), so Loop bounces it |
| Keep the reverb tail | ◐ | ● | ● | ● | **Works**: a tail of up to 30 s past the range, set in the dialog (`tail`) |
| WAV, AIFF, FLAC, MP3 and a modern codec | ● | ● | ● | ● | **Works**: WAV, AIFF, FLAC, MP3, Opus |
| Convert sample rate on export | ● | ● | ● | ● | **Missing** for WAV, AIFF and FLAC |
| Dither with a choice of type and noise shaping | ● | ● | ● | ● | **Partial**: TPDF, always on, no control in the dialog |
| Mono, and one file per channel | ● | ● | ● | ● | **Missing** |
| Normalize on export | ○ | ◐ | ● | ◐ | **Works**: to sample peak (−0.1 dBFS) or to a loudness target in LUFS held under a true-peak ceiling, by rendering twice |
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
| Dolby Atmos objects with a built-in renderer | ● | ● | ● | ● | **Not planned here** (the owner's call); the object switch stays a label |
| Binaural monitoring | ● | ● | ● | ● | **Not planned here**: needs a measured head-related impulse response set, content the owner sources |
| ADM BWF export | ● | ● | ● | ● | **Not planned here**: ADM BWF is the Atmos master's container and carries objects; with Atmos objects and a renderer not planned it has nothing to carry beyond a bed, which a WAV with a speaker mask already does |
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
| Snapshots or scenes of the whole mix | ○ | ● | ◐ | ● | **Works**: Window › Mix Snapshots stores every strip under a name and recalls it whole or by section (faders, pan, mutes, sends, inserts, routing, delay), on every track or the selected ones (`mix.snapshot_store`, `mix.snapshot_recall`), one undo step each; saved with the session |
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
| Automation playback: breakpoints honoured inside a block, sends, mute and plug-in values ramped, all of it delay-compensated, identical in a bounce | 14–22 | Closes "interior breakpoints" in `gaps.md` G-04. **Done for the fader section** (volume, trim, VCA, mute, pan, sends); plug-in values are delay-compensated (2026-11-16) but still change once per block: reading them sub-block needs the plug-in interface to take a block in pieces (or timed parameter events), which the built-in `Plugin::process` does not |
| Automation writing: controls follow automation, you hear what you move, written at the heard position, return time on release, thin after a pass | 10–16 | **Thin after a pass, faders and pan knobs following, writing at the heard position and a fixed return time done**; hearing the moved control live remains |
| Automate plug-in bypass, instrument parameters and surround pan; make the per-type enables real | 6–10 | **Per-type enables done**; bypass, instrument and surround pan lanes remain |
| Key inputs: on the stock compressor, gate and de-esser, and sidechain busses for CLAP, VST3 and AU | 12–20 | Part of G-13. **CLAP keys and the generic routing done**; VST3 and AU key buses, Comp's own detector (effects work), remain; a key taken from an aux by an ordinary track is done (built from the design below) |
| Controller mapping (MIDI learn) for faders and plug-in parameters | 8–12 | **Done** (section 6, K): faders, pan, send levels and chain knobs; Mackie Control and HUI surfaces too |
| Metering: pre-fader option, RMS and scale choice, a loudness and true-peak meter on the master, output meter without the click | 8–12 | **Done**: RMS, VU and K scales, and Window › Loudness Meter; PPM and loudness range remain |
| Pan law choice; balance and width for stereo; per-track delay | 6–10 | **Done**, width and balance automatable too |
| Sends to tracks and hardware outputs, a return made with a new send, default send level, loop refusal | 6–10 | Shared with section 2's cue mixes. **Done**, with pan following the main pan and pre-fader sends that can ignore mute |
| Bounce dialog: source, tail, sample rate, dither and mono options, metadata, add to session; stems as a background job with a choice of tracks and busses | 12–18 | **Source, tail and stems as a background job done**; the rest remains |
| Real-time bounce | 4–6 | |
| Group attributes, suspend all, VCA assignment in the mixer, nested VCAs | 6–10 | **Done** |
| Atmos: objects, a renderer of our own, binaural monitoring, ADM BWF | 50–80 | Is G-12; listed here because all four now have it, sized there |

**3. What most have (≈ 40–65 h).** Mixer snapshots (Studio Pro, Cubase), 8–12 (**done**); a listen bus for
AFL and PFL (all but Logic), 6–10 (**done**, with the control room); post-fader inserts and plug-in deactivation (**done**), 5–8; hardware
inserts with a latency ping, 6–10, hardware needed; multi-mono processing, 6–10; write-on-stop and
preview (Pro Tools, Cubase), 5–8; folders without an audio path, mixer search and spill, 5–8.

### The control room (tier 3, built 2026-11-16 to this design)

**Built:** `ControlRoom` in the session (`crates/model/src/control_room.rs`), the engine's monitor
path, listen bus, cue levels and talkback (`crates/mix/src/monitor.rs`), the `monitor.*` commands
(`crates/engine/src/cmd/monitor.rs`) and Window › Control Room (`crates/ui-egui/src/control_room.rs`,
F11 held to talk). Differences from the design: sets and cues name their output (as routes do)
rather than index it; a cue's sends are ordinary sends to its output (Add Cue Sends makes them,
pre-fader, at the track's fader level), not extra slots; switching sets cuts rather than
crossfades (level, dim, mono, mute and talkback ramp over 10 ms); talkback is not latched into the
monitor speakers. Not verified by listening or on hardware outputs. Tests: `mix` `tests/control_room.rs`,
`engine` `monitor::tests`.

The design as written:

What Cubase's Control Room and Pro Tools' monitoring paths give an engineer, on FreeDAWm's
existing pieces: hardware outputs (`Session::outputs`), sends that now reach a hardware output,
pre-fader sends that can ignore mute, and the solo scope (`SoloScope`).

- **Model.** A `ControlRoom` in the session (absent: today's behaviour, nothing changes): the
  *monitor* output (the main mix's speakers, an index into `outputs`), a list of *monitor sets*
  (`{name, output, level_db, trim_db}`: alternative speakers; one active), the *listen bus*
  (`{output?, level_db, mode: afl|pfl}`), *cue mixes* (`{name, output, level_db, talkback: bool}`)
  and *talkback* (`{input: InputPath name, level_db, latch: bool}`), plus `dim_db` (−20 default),
  `dim`, `mono`, `mute` switches. Saved with the session; all of it commands (`monitor.*`).
- **Listen bus (AFL/PFL).** `options.solo_mode` `afl|pfl` stops being a flag: in those modes a
  soloed track is not soloed in place; its post-fader (AFL, after pan) or pre-fader (PFL, mono-
  summed or as is) signal is summed into a listen buffer in `route_strip` beside the sends, and
  the monitor path plays the listen buffer instead of the main mix while any listen is active
  (Pro Tools' "listen replaces the mix"). The main mix, bounces and cue mixes are untouched.
- **Cue mixes.** A cue is a hardware output fed by sends (the send menu's Output ▸ already does
  this); the control room adds one send slot per cue on every track (made on demand, pre-fader by
  default, starting from the track's fader with "Copy fader to cue"), a level per cue, and the
  talkback summed in when its switch is on.
- **Talkback.** The talkback input is read from the live input feed (it is never recorded), at
  its level, into each cue whose talkback is on (and the monitor path, dimmed, when latched).
- **Monitor sets.** The monitor path is the main mix (or the listen buffer) copied to the active
  set's output after the master faders: level and trim, dim, mono (L+R)/2, mute, with 10 ms ramps.
  The main output keeps feeding Bounce and the meters before this stage, so none of it changes a
  render. Switching sets crossfades.
- **Where.** A Control Room window (Window ›), the master strip's monitor knob, and the commands
  for control surfaces. Engine work sits in `MixEngine::render` after the master stage; the
  player needs no new channel (the outputs already are hardware buffers).
- **Tests.** Offline: the listen buffer carries only the soloed strip; AFL follows the fader, PFL
  does not; dim and mono act on the monitor output only; a bounce is identical with the control
  room on and off; talkback reaches a cue and never the main mix.

### Design: an ordinary track keyed from an aux or bus (owner decision 5, built)

Today strips render in two phases: the non-aux tracks in levels (a key source renders in an earlier
level, `sidechain::resolve`), then the auxes and folders, serially, each after the busses feeding it
are summed. A module on an ordinary track keyed from an aux is refused, because that aux has not
rendered when the track does.

- **Order.** Replace the two phases with one dependency order over all strips: an edge from every
  strip to the busses it writes (output, sends), from a bus to the strips reading it (input), and from
  a key source to the keyed strip. A topological sort gives the order; strips of one level still
  render in parallel. Today's sessions sort into today's order (auxes have only bus edges in).
- **Loops.** A key from an aux whose input is fed, directly or through other busses, by the keyed
  track is a loop: refused at `mix.module_sidechain` with the path named, as routing loops are
  (`Session::route_loops`); a session that has one (an older file) plays the key silent, as a
  regressed key does now.
- **What the keyed track may feed.** Its output and sends go to busses read after it; a bus read by
  an aux that the order has already rendered would be one block late, which the sort rules out
  (that aux would depend on the track).
- **Delay compensation.** The key is read where the aux's output is: the aux's `post_at` (its bus
  latency plus its inserts), as keys from tracks are read at theirs; `fill_key`'s alignment line
  delays whichever side is earlier, so nothing new is needed beyond feeding it the aux's position.
  The aux's own compensation (`aux_latency`) is fixed before the keyed track renders, so the
  track's `out_extra` still lines its output up with the other returns.
- **Tests.** An ordinary track keyed from an aux is ducked by it, sample-aligned with and without
  latency on the aux; a loop is refused; bounce and playback agree.

**As built.** The two phases stay, so today's sessions render exactly as before: a track that an
aux reaches (keyed from it, or keyed or fed MIDI by a track that is) is *late* (`Plan::late` in
`mix/src/sidechain.rs`) and renders in the second phase, ordered with the auxes by the same ranks,
after its key source and before the auxes reading the busses it feeds. A loop is the existing
`Cycle` refusal (`mix.module_sidechain` and the picker grey it out). Delay compensation: a late
track's latency is left out of `track_latency`, or the busses feeding its aux would wait for a key
that waits for them; it lines up at the main mix's position (the tracks' compensation plus the
slowest aux return) with no extra output delay. Not covered: a keyed track's own plug-in latency
(a look-ahead) beyond that key wait is not compensated against the rest of the mix (the stock
compressor has none), and its output onto a bus no aux returns onto is late by the slowest aux
return when an aux has latency. Tests: `an_ordinary_track_keyed_from_an_aux_lines_up_with_the_mix`,
`a_key_from_an_aux_the_track_feeds_is_a_loop`, `the_stock_compressor_ducks_a_track_from_an_aux`
(`mix/tests/sidechain_edge_cases.rs`). Bounce and playback run the same render, so they were not
compared separately.

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
| 2026-11-17 | minor | polarity per channel with a button on the strip; Copy to Send takes the selection, pan and mute |
| 2026-11-17 | minor | rows reconciled with the code: the stock compressor's key, delay-compensated automation, surface writing, the meter plug-ins, the bounce range and normalising were already built; Atmos objects, binaural and ADM marked not planned here |
| 2026-11-16 | minor | an ordinary track can be keyed from an aux or bus return (owner decision 5), built from the design |
| 2026-11-16 | minor | stems ignore an AFL or PFL solo, as a bounce does (owner's decision) |
| 2026-11-16 | minor | width and balance lanes; a design for keying an ordinary track from an aux |
| 2026-11-16 | minor | plug-in automation is read where the module's audio is (delay-compensated) |
| 2026-11-16 | minor | tier 3: post-fader inserts |
| 2026-11-16 | minor | tier 3: the control room (listen bus, monitor sets, cue mixes, talkback) |
| 2026-11-16 | minor | MIDI learn for faders, pan, send levels and plug-in knobs |
| 2026-11-16 | minor | Control surfaces (Mackie Control, HUI) write automation from their faders and V-Pots |
| 2026-11-16 | minor | tier 3: a control room design |
| 2026-11-16 | minor | tier 3: mixer snapshots |
| 2026-11-16 | minor | tier 2: group attributes in the dialog, nested VCAs |
| 2026-11-16 | minor | tier 2: sends to outputs and tracks |
| 2026-11-16 | minor | tier 2: width, balance and track delay |
| 2026-11-16 | minor | tier 2: RMS and K scales, a master loudness meter |
| 2026-11-16 | minor | tier 2: CLAP plug-ins take a key |
| 2026-11-16 | minor | tier 2: a Touch control let go glides back |
| 2026-11-16 | minor | tier 2: moves are written where they were heard |
| 2026-11-16 | minor | tier 2: VCA assignment from the track menu |
| 2026-11-16 | minor | tier 2: the Automation window's write enables work |
| 2026-11-16 | minor | tier 2: bounce a bus, with a tail |
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
