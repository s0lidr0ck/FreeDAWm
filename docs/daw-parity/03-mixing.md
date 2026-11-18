# Four-DAW parity, section 3: mixing

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-18 · **Change:** minor (round three: strip, solo, bounce, automation and metering rows built) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

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
| B. Panning | Four pan laws, dual pan with link and mirror, width and balance for stereo | Small: no binaural |
| C. Inserts and sidechains | 32 modules, drag to reorder and copy, chain presets, CLAP key inputs | Medium: no key on AU |
| D. Sends and routing | 10 sends to busses, tracks or outputs, busses of any format, routing folders | Small: one output per track |
| E. Groups, VCAs, solo | Path-aware solo in place, AFL/PFL on a listen bus, solo safe, momentary solo, relative groups with a choice of attributes, nested VCAs | Small |
| F. Automation | All modes write; trim lane; wide editing set; fader section sample-accurate and delay-compensated; plug-in values delay-compensated, hosted plug-ins' sample-accurate; faders follow | **Large**: built-in plug-ins block-rate |
| G. Metering | Peak with hold and clip lights, no peak missed, pre- or post-fader, RMS/VU/K/PPM scales, a master loudness meter with loudness range | Small |
| H. Mixdown and export | Offline and real-time bounce of the mix, a bus or a hardware output to five formats in one pass, with a tail, rate conversion, dither choice, mono or split files, tags and Add to Session; stems of chosen tracks and busses; DAWproject | Small: no AAF or OMF |
| I. Surround | Beds to 9.1.6, a surround panner, fold-down | **Large**: all four now render Atmos objects and binaural |
| J. Mixer state and window | Undo for every move, track and chain presets, per-track delay, snapshots, find and spill | Small |

Two things every one of the four does and FreeDAWm does not yet: **plug-in automation that is
sample-accurate** (the fader section's now is, and the faders follow it; plug-in values are delay-compensated but block-rate); and **a key input on
AU plug-ins** (CLAP and VST3 plug-ins and the stock compressor and gates take one). Meters now have RMS, VU and
K-system scales, and the master a loudness meter.

## A. Channels and the strip

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Audio, aux, instrument, MIDI, master, VCA, folder channels | ● | ● | ● | ● | **Works** |
| Folder that only organises (no audio path) | ● | ● | ● | ● | **Works**: Move to New Basic Folder in a track's menu (`track.move_to_new_folder` `routing: false`): a folder with no bus and no input or output; its members keep their outputs as they move in and out; it shows, hides, collapses, selects and spills them as a routing folder does. Its strip passes no audio, so its fader, mute and solo do nothing (as Pro Tools' basic folders have none) |
| Formats from mono to 7.1.4 and wider | ● | ● | ● | ● | **Works** up to 16 channels (3rd-order Ambisonics); orders 4–7 are refused with a message (`track.new`, `track.change_width`, buses), not truncated |
| Change a track's format later | ◐ | ● | ● | ◐ | **Works** |
| Input trim and polarity on the strip | ● | ● | ● | ● | **Works**: trim, and a Ø button beside it on the strip's preamp row: a click inverts every channel, a right-click single ones (`mix.phase_invert` `channel`) |
| Fader to +12 dB | ● | ◐ | ◐ | ◐ | **Works** |
| Per-track delay in samples or ms | ● | ● | ● | ● | **Works**: Track › Strip › Delay (`mix.track_delay`, samples or ms, up to 192 000 samples), before the inserts, left in by delay compensation |
| Built-in channel processing (EQ, dynamics or console sound in the strip) | ◐ | ◐ | ○ | ● | **Not planned here**: the owner keeps the hidden stock EQ and compressor hidden (owner decision 4); a strip's EQ and dynamics are inserts (CleanQ, Comp), which a track preset or a chain preset puts on every new track |
| Make a track inactive to free its plug-ins | ● | ● | ● | ● | **Works** (2026-11-18): an inactive track's inserts and instrument hold no instances: Make Inactive first stores every hosted plug-in's live state on its insert (as a save does), then the engine retires the instances (dropped off the audio thread, as a removed module's are), asks for no hosted instance and prepares no built-in one for it; made active again (or by Undo) they are made anew from the stored state and parameters and it sounds as before. A session opened with inactive tracks no longer loads their plug-ins at all. A render through the live instances (Bounce, Freeze) keeps them while it runs, since its sessions make the tracks it does not render inactive; Clip Effects modules on the track's clips stay loaded (they are built-ins) |

## B. Panning

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Choice of pan law | ● | ● | ● | ● | **Works**: 0, −3, −4.5 or −6 dB, per session (Setup › Preferences… › Mixing › Pan Law, `mix.pan_law`) |
| Dual pan for stereo tracks | ● | ● | ● | ● | **Works** |
| Balance, and width or combined panner | ○ | ● | ● | ● | **Works** for stereo tracks: Track › Strip › Width and Balance (`mix.width`, `mix.balance`) on the main output, around the dual pans; automatable (width and balance lanes, on the pan's 64-sample grid; moving the controls counts as a touch for writing, not yet tried in a writing pass) |
| Link or mirror left and right pans | ● | ◐ | ◐ | ● | **Works**: Track › Strip › Pans: Independent, Linked (the other pan moves by as much, the same way) or Mirrored (the other way, about the centre) (`mix.pan_link`), for every way a pan moves (knobs, surfaces, MIDI learn, groups); saved with the session |
| Surround panner | ● | ● | ● | ● | **Partial**: position, divergence and height on the panner, centre % and LFE level in the strip name's menu (Surround ›); mono and stereo tracks only (a wider track is not panned) |
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
| Compare (A/B) in every plug-in window | ● | ● | ● | ● | **Works** (2026-11-18, f2146c7): every plug-in window and drawer has A/B: the built-in editors and drawers swap parameter values; a hosted plug-in (CLAP, VST3) swaps its whole state, read from the running instance (`Player::capture_state`: CLAP state, VST3 component and controller state; the stored state when it is not running) and loaded back as a hosted preset is (`mix.module_preset`, the instance made again from it, one undo step per flip). The hosted window shows an A/B button beside its editor button, and its Presets menu's Compare does the same. B starts as a copy of A. What needs a plug-in installed and listening: a flip makes the instance again, so a plug-in that takes long to load is heard after it is ready |
| Key input on the stock compressor, gate and de-esser | ● | ● | ● | ● | **Works**: Comp (every style, its detector measures the key), the gates Lull and Shale, and Rez take a key; the routing side is generic (any plugin whose `PluginInfo::sidechain` is set gets the Sidechain row in its insert header and a delay-compensated key from any track); any track can be keyed from an aux or bus return (it renders after that aux; a key from an aux the track feeds is refused as a loop) |
| Key input on third-party plug-ins | ● | ● | ● | ● | **Partial**: CLAP plug-ins with a second input port and VST3 plug-ins with an auxiliary audio input bus are keyed (from the insert header, delay-compensated; a VST3's first aux input is activated and fed the key, a mono key on each of its channels); Audio Units are not (macOS only, not attempted here: no Mac to build or test on) |
| Key aligned by delay compensation | ● | ● | ● | ● | **Works** |
| Multi-mono or dual-mono processing | ● | ◐ | ● | ◐ | **Works** (2026-11-18): "Multi-Mono" on a chain module's right-click menu (`mix.module_multi_mono`, `Insert::multi_mono`) runs one mono instance per channel of a wider track, built-in or hosted, all linked to the module's parameters, automation and stored state (the instances are made with the module's, off the audio thread, by the stereo-only wrapper `PerChannel` with a first instance of one channel). "Multi-Mono Unlinked" beside it (8b7075a; `linked: false`, `Insert::multi_mono_unlinked`) gives each channel past the first a parameter set of its own, stored with the module's under `c<n>:<param>` (`c1:gain` is the second channel's), starting from the module's values, with lanes of those ids listed per channel in the lane picker; the plug-in window gets a channel picker (`mix.module_channel`: every editor then shows and edits that channel) and a Link button. Linking again keeps the channels' values (not heard) for unlinking later. Round five: unlinked, a hosted plug-in's stored state holds every channel's own (packed by the wrapper, `multi_mono_pack_states`; saved through a reader of every instance, restored channel by channel, and a plain preset loaded into the running module goes to the first channel as a plain parameter does); linked again, or made a single instance, the module takes the first channel's (test `unlinked_multi_mono_saves_and_restores_each_channels_state`). A built-in's derived parameters (Dist's Link and per-circuit knob memory, Mod's Link) run on each channel's own values: an edit of channel n derives into its `c<n>:` values, with its own memory under `c<n>:` keys, in the same undo step (test `an_unlinked_channel_derives_its_own_parameters`). Existing sessions are unchanged (nothing packed unless a module is unlinked) |
| Hardware inserts with latency measurement | ● | ● | ● | ● | **Works** in code (2026-11-17, to the design below): Utilities › Hardware Insert (`hw_insert`) sends the track out of an output path and takes it back from an input path, compensated by its round trip; Ping in its drawer measures that round trip on its own paths (`setup.measure_latency` with `insert`) and stores it. Offline it passes through, its latency kept (a real-time bounce prints the gear). Tested with a loopback in code; hearing gear through it and checking the ping lines it up need an interface with a cable or outboard unit from output to input. Not measured, it compensates nothing (the design's fallback to the device estimate is not built), and since 2026-11-17 (bb2fc6e) its slot says "not measured" (dimmed, with the reason on hover) and its drawer says no delay compensation is applied until it is pinged |

## D. Sends and routing

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Eight or more sends, pre or post fader | ● | ● | ● | ● | **Works**: ten |
| Send level, pan and mute, automatable | ● | ● | ● | ● | **Works**: a stereo send's pan balances it; Pan Follows Main in the send's menu makes it take the track's pan and its automation |
| Send to a bus, a track or a hardware output | ● | ● | ● | ● | **Works**: a send's menu offers busses, Output ▸ (straight to a hardware output) and Track ▸ (the track's input becomes the bus the send feeds) (`mix.send` `output`, `to_track`) |
| A new send comes with its return | ● | ● | ● | ● | **Works**: a send to a new bus from the Mix window makes its aux (`mix.send` `return: true`) |
| Default send level setting | ● | ◐ | ● | ◐ | **Works**: silent or 0 dB (Setup › Preferences… › Mixing › Default Send Level; any level by `mix.default_send_level`) |
| Pre-fader sends that ignore mute, as an option | ● | ◐ | ● | ● | **Works**: Setup › Preferences… › Mixing › Pre-Fader Sends Ignore Mute (`mix.pre_sends_ignore_mute`), also through a solo's mute |
| Copy the fader into a send (for cue mixes) | ● | ● | ◐ | ● | **Works**: Edit › Automation › Copy to Send copies the volume, and pan and mute when asked (`automation.copy_to_send` `what`), over the edit selection when there is one (what the send had there is replaced, its curve either side kept), else the whole lane; without automation the values themselves; the Control Room's Add Cue Sends makes a pre-fader send to a cue on every track at its fader level (`monitor.cue_sends`) |
| Busses of any format, output to bus or hardware | ● | ● | ● | ● | **Works** |
| Several outputs from one track | ● | ○ | ○ | ◐ | **Works**: Add Output in a strip's output selector (`mix.send` `as_output`): another bus or hardware output beside the main one, a post-fader copy at the fader's level with the main pan and its automation, silenced by the track's mute, whatever the slot's own level, pan, mute and lanes; listed there with Remove. It takes one of the ten send slots (Pro Tools' additional outputs do not); it is shown in its send slot like a send |
| Refuse or warn about a feedback loop | ● | ● | ● | ● | **Works**: refused, for outputs, inputs and sends |
| Routing folders, nested | ● | ● | ● | ● | **Works** |

## E. Groups, VCAs and solo

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Groups that link volume, mute, solo and arm | ● | ● | ● | ● | **Works**, relative offsets kept |
| Choose what a group links (pan, sends, inserts, automation mode) | ● | ◐ | ● | ● | **Works** for volume, mute, solo, record, pan, sends and automation mode, in the Group dialog (Create and Modify) and by command; inserts are not linked |
| Suspend all groups | ● | ● | ● | ● | **Works**: Options › Suspend All Groups |
| VCA masters | ● | ● | ● | ● | **Works**: assigned from the track menu (VCA ▸, in the Edit and Mix windows), nested up to eight deep (a VCA follows another; loops refused), each one's fader, mute and solo reaching the members below |
| Solo in place that follows the signal path | ● | ● | ● | ● | **Works**: effect returns of a soloed source are heard |
| Solo safe | ● | ● | ● | ● | **Works** |
| Exclusive solo | ● | ● | ● | ● | **Works** |
| AFL / PFL on a listen bus | ● | ● | ○ | ● | **Works**: in the AFL and PFL solo modes (Options › Solo Mode, or the Control Room) a solo mutes nothing; the soloed tracks are summed after the fader and pan (AFL) or before the fader (PFL) into a listen bus that replaces the mix on the monitor path while any is soloed, at its own level; the mix, bounces, stems and cue mixes are untouched (stems ignore an AFL or PFL solo, the owner's decision: each stem is its own track as with no solo) |
| Momentary solo | ● | ○ | ○ | ○ | **Works**: Options › Solo Mode › Momentary (Hold to Solo) (`options.momentary_solo`): an S key solos its track (with the click's modifiers' scope) while the mouse button is held on it, in the Mix and Edit windows; letting go anywhere puts it back, and a latched solo elsewhere is left alone |

## F. Automation

### What can be automated

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Volume, pan, mute, send level, pan and mute | ● | ● | ● | ● | **Works** |
| Plug-in parameters, stock and third-party | ● | ● | ● | ● | **Works** for inserts |
| Instrument parameters | ● | ● | ● | ● | **Works**: built in the MIDI writing work (33c96ba): an instrument's parameters take lanes as an insert's do (listed under the instrument's name in the lane picker, `plugin:instrument:<param>`), played into built-in and hosted instruments and written by Touch, Latch and Write |
| Plug-in bypass | ● | ● | ● | ● | **Works**: each module has a Bypass lane (in its group of the lane picker; `plugin:m<id>:@bypass`, 1 bypassed): read where the module's input is, crossfaded as a click is, written in a pass when the module is bypassed or switched back on |
| Surround pan | ● | ● | ● | ● | **Works**: left-right, front-back, height and divergence lanes (`surround_x`, `_y`, `_z`, `_divergence`) on a track with a surround panner, read on the pan's 64-sample grid and by sends that follow the main pan; written when the panner moves in a pass |
| VCA and master fader | ● | ● | ● | ● | **Works** |

### Writing and playing it

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Read, Touch, Latch, Write | ● | ● | ● | ● | **Works**, one undo step per pass |
| Trim automation | ● | ○ | ● | ● | **Works** for volume |
| Faders, knobs and plug-in controls move with automation | ● | ● | ● | ● | **Works**: the Mix window's faders, pan knobs and sends (level, pan, mute) and the built-in modules' drawers and full editors follow their lanes while the track reads and the control is not touched; a hosted plug-in's own window shows what the host sends it |
| You hear the control you are moving | ● | ● | ● | ● | **Works** (2026-11-18): while a pass writes a lane (a control held or latched in Touch, Latch or Touch/Latch, every lane in Write) that lane is held out of the session the player plays, as Preview holds one out, so the strip plays the control as set: the move reaches the audio thread with the next block after the frame that made it, not 20 ms of written points later. Let go in Touch, the lane plays again (the glide back is written as before); a Latch plays the latched value until Stop; stopped, every lane plays. The trim a fader holds in Trim mode still plays through its lane |
| Sample-accurate, or at least block-independent | ● | ● | ● | ● | **Works**: volume, trim, VCA and mute are read sample by sample where a lane bends or breaks inside a block; a send's level is read sample by sample too, and pan (pan, width, balance, surround) and a send's pan are read at the 64-sample timeline grid's points with every routing gain gliding in a straight line between them, so a lane that bends inside a block moves the image every sample instead of in 64-sample steps; a send's mute still switches on the grid, and into a bus with a summing model sends and pan still step on it. Where lanes are flat (or absent) the mix is bit for bit what it was. Everything is read by timeline position, so playback and a bounce at any block size match to the bit. Hosted CLAP and VST3 plug-ins get a lane that moves inside a block as timed parameter events on the 64-sample grid (`Plugin::set_param_at`); of the built-in plug-ins, Gain, Trim, Channel Tool, CleanQ, Filter and Comp take them too (run in pieces split at each change, section 5), the others still change once per block. Cost, measured in a debug build on a shared machine (noisy): 16 mono tracks each with a send, three bending lanes per track, about 350–600 µs per 256-frame block against 250–290 µs on the grid before; flat lanes about as before. Not verified by listening. Tests: `mix` `a_bending_pan_lane_glides_sample_by_sample_at_any_block_size`, `a_bending_send_level_glides_sample_by_sample_at_any_block_size`, `flat_pan_and_send_lanes_mix_as_the_stored_values`; the measurement `cost_of_gliding_pans_and_sends` (ignored) |
| Automation delay-compensated | ● | ● | ● | ● | **Works**: fader, mute, pan and send automation play where their audio is (behind by the strip's plug-in delay), and a module's parameters where its input is (behind by the modules before it), in playback and bounce alike; moves are written where they were heard (the mix delay and output latency behind the transport) |
| Glide back to the old curve on release (return time) | ● | ● | ● | ● | **Works**: Touch glides back over the return time, 250 ms unless set in the Automation window (`automation.return_time`, 0 to 5 s) |
| Write to start, end or the whole selection on stop | ● | ○ | ○ | ● | **Works**: Write on Stop in the Automation window (`automation.write_on_stop`: to start, end, all or the selection), armed for the next pass as Pro Tools arms it: when it stops, each lane it wrote keeps its last value over that range, in the pass's undo step |
| Preview a value before committing it | ● | ○ | ○ | ● | **Works**: Preview in the Automation window (`automation.preview`): a control touched while it is on plays as set, its lane suspended (not saved) and not written by passes; Punch Preview (`automation.punch_preview`) writes each previewed value over the edit selection, or from the position heard to the end, in one undo step, and the lanes play again; Preview off without a punch drops the values. Pro Tools' Suspend Preview and capturing values into a preview are not built |
| Enable or suspend by parameter type | ● | ◐ | ◐ | ● | **Works**: the Automation window's switches (`automation.enable`) stop a type being written; its lanes still play |
| Write from a MIDI controller or control surface | ● | ● | ● | ● | **Works**: a Mackie Control or HUI surface's touch-sensitive faders write as the on-screen fader held does (Touch, Latch), its V-Pots write pan (section 6, K); MIDI learn maps a controller to a fader, pan, send level or plug-in knob, written as the on-screen control moved |
| Automation follows tempo changes | ● | ● | ● | ● | **Works** on tracks kept in bars and beats (tick timebase, the default for MIDI and instrument tracks): a tempo edit moves each lane it left alone to the same ticks in the new map, as their clips move (`tick_clips::follow_tempo`); a sample-timebase track's points stay on their samples |

### Editing it

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Add, move, delete, nudge points | ● | ● | ● | ● | **Works** |
| Curved segments | ○ | ● | ● | ● | **Works**: one bend per segment |
| Pencil shapes (line, triangle, square, random) | ● | ● | ◐ | ● | **Works**: right-click the Pencil in the toolbar for Free Hand, Line, Triangle, Square or Random (`automation.pencil_shape`); a shape goes between the press's value and the pointer's, repeating at the grid's step (`automation.draw` `shape`, `period`); Shift still draws a line |
| Cut, copy, paste, paste to another parameter | ● | ● | ● | ● | **Works**: curves and the range's edges come along; a paste replaces what was there |
| Thin, and thin automatically after a pass | ● | ● | ◐ | ● | **Works**: a pass's lanes are thinned when it ends (Setup › Preferences… › Mixing › Thin Automation After Pass) |
| Automation moves with clips | ● | ● | ● | ● | **Works**; the curve changes shape at the clip edges |
| Automation inside a clip or region | ○ | ◐ | ● | ◐ | **Works** (2026-11-18, to the design below): lanes stored on the clip (`Clip::automation`, saved only when not empty) move, trim, split and copy with it and are heard over its range in playback, bounces and freezes (dB lanes add to the track's, others replace it, a 64-sample ramp at the edges), from flattened lanes the engine makes after each edit (`Track::played`, `clip_automation`). Edit › Automation › Move Track Automation into Clips, Move Clip Automation to Track and Clear Clip Automation. The Edit window draws them (6ca37e7): over each clip with a lane of its own, the track's automation lane shows the lane as it plays there (accent colour, the clip's range shaded) with the clip's points on it; and edits them by mouse there (07767c9) as the track's points are edited: a drag moves a clip's point between its neighbours inside the clip (one undo step), Alt-click deletes it (the lane goes with its last point), and a Pencil click or a Smart/Grabber double-click inside a clip that has a lane of its own for the parameter adds the point to the clip, not the track (on a dB lane the clip stores the difference from the track's line, so the point lands where it is clicked); `clip.automation_point` (`from` moves a point) and `clip.automation_delete`. A clip with no lane yet gets its first points by command or by Move Track Automation into Clips |
| Several lanes per track, any parameter | ● | ● | ● | ● | **Works** |
| Show the parameter last touched | ◐ | ● | ● | ● | **Works**: Edit › Automation › Show Last Touched (`automation.show_last_touched`) selects the track whose control (fader, pan, send, mute or plug-in knob) moved last and shows that lane; `automation.last_touched` names it |
| Convert between clip gain and volume automation | ● | ○ | ○ | ○ | **Works** |

## G. Metering

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Peak meters that catch every peak | ● | ● | ● | ● | **Works**: each screen update shows the loudest block since the last one |
| Peak hold and clip lights | ● | ● | ● | ● | **Works**; one bar per channel on surround tracks |
| Pre-fader metering | ● | ● | ● | ● | **Works**: Setup › Preferences… › Mixing › Pre-Fader Metering; track strips only (the master meter stays post-fader) |
| RMS, and a choice of scales (VU, K-system, PPM) | ● | ● | ◐ | ● | **Works**: Preferences › Metering › Scale: peak, RMS (300 ms), peak with RMS inside, VU (0 VU = −18 dBFS), K-20, K-14, K-12, numbered from their reference; release and peak hold beside it; saved with this computer's settings. PPM (EBU, 0 = −18 dBFS) falls 24 dB in 2.8 s as EBU Tech 3205 times it; its rise follows the sample peak (no 10 ms integration) |
| Gain reduction on the strip | ● | ● | ● | ● | **Works**: a number for the first dynamics module |
| Loudness (LUFS) and true peak while mixing | ◐ | ● | ● | ● | **Works**: Window › Loudness Meter reads the master as it plays: momentary, short-term, integrated, loudness range (EBU Tech 3342, from the short-term value every 100 ms), maximum momentary and true peak, with Reset |
| Correlation, spectrum, vectorscope | ● | ● | ● | ● | **Works**: the Spectrum and Phase Meter plug-ins (Utilities in the insert menu), as well as inside the Sprawl and CleanQ modules and Scout |
| Output meter free of the click | ● | ● | ● | ● | **Works** |
| Meters while stopped | ● | ● | ● | ● | **Works**: armed and monitored tracks and live MIDI meter while stopped, and a Stop (or the end of play) lets the mix ring out, heard and metered, until it has been quiet 50 ms (at most 4 s, then faded); a second Stop cuts it. A stop while recording still cuts at once |

## H. Mixdown and export

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Offline bounce, with progress and cancel | ● | ● | ● | ● | **Works**, through the live plug-in instances |
| Real-time bounce (for outboard gear) | ● | ● | ● | ● | **Works** (2026-11-17, to the design below): Bounce Mix › "In real time" plays the range through the audio device and prints the main mix or a bus as it plays (`playback::print`, after the master's chain, without the click or the monitor stage), then writes the print through the offline bounce's writer, so every option applies. Editing waits behind the progress window; Cancel stops it; a pass that falls behind or is stopped writes nothing. Held in memory, so as long as Consolidate allows (about 93 min stereo at 48 kHz). Tested against a test device and against the offline bounce; through outboard gear needs hardware |
| Choose the source: any bus or output | ● | ● | ● | ● | **Works** (2026-11-18): the main mix, any bus, or a hardware output of the I/O Setup (Bounce Mix's Source menu lists them as "Output: name"; `file.bounce_mix` takes `source` as a bus or output name, or `output:name` when a bus has the same name). An output is bounced as it goes to the device: what the tracks and sends routed to it play, after the wait for the master faders' inserts, lined up with the timeline by the main mix's latency, offline (`RangeRender` over a `Tap`) and in a real-time bounce alike |
| Range: selection, loop, markers, whole session | ● | ● | ● | ● | **Works**: the selection is the loop range (one thing in FreeDAWm), so Loop bounces it |
| Keep the reverb tail | ◐ | ● | ● | ● | **Works**: a tail of up to 30 s past the range, set in the dialog (`tail`) |
| WAV, AIFF, FLAC, MP3 and a modern codec | ● | ● | ● | ● | **Works**: WAV, AIFF, FLAC, MP3, Opus |
| Convert sample rate on export | ● | ● | ● | ● | **Works**: Sample Rate in the Bounce dialog (`sample_rate`, 8 to 384 kHz), converted as the blocks are written (the windowed-sinc converter the MP3 and Opus writers use), for every format |
| Dither with a choice of type and noise shaping | ● | ● | ● | ● | **Works**: Dither in the Bounce dialog: TPDF, noise-shaped TPDF (second-order error feedback, the noise moved above the midrange) or off (`dither`), for 16- and 24-bit files |
| Mono, and one file per channel | ● | ● | ● | ● | **Works**: Channels in the Bounce dialog: as mixed, mono (the channels averaged; a surround source's stereo fold-down) or one mono file per channel named after it, `Mix.L.wav`, `Mix.R.wav` (`channels`) |
| Normalize on export | ○ | ◐ | ● | ◐ | **Works**: to sample peak (−0.1 dBFS) or to a loudness target in LUFS held under a true-peak ceiling, by rendering twice |
| Tags and broadcast metadata in the file | ● | ● | ● | ● | **Partial**: every bounced WAV carries a BWF `bext` (FreeDAWm, the date, the range's start as its time reference, the title as its description); Tags in the Bounce dialog (title, artist, album, date, comment; `tags`) go into an MP3 as ID3v2 and a WAV as `LIST`/`INFO`. FLAC, AIFF and Opus files are not tagged yet |
| Add the bounce back to the session | ● | ● | ● | ● | **Works**: Add to Session in the Bounce dialog (`import`): the main file (each file of a split bounce) on new tracks at the range's start, one undo step |
| Several formats in one pass | ● | ◐ | ● | ● | **Works**: Also write in the Bounce dialog (`also`): more formats from the same render, each beside the main file with its own extension (MP3 and Opus at their default bitrate; a surround mix needs a fold-down or a split for them) |
| Stems in one pass, with a choice of tracks or busses | ● | ● | ● | ● | **Works**: Export › Stems takes every unmuted track or the selected ones, and a stem of each bus as summed (`tracks`, `busses`), behind a progress window with Cancel; a track's stem is it soloed through the master |
| Commit, freeze, bounce in place | ● | ● | ● | ● | **Works**: commit keeps the format, the pan and the fader automation |
| AAF or OMF | ● | ● | ● | ● | **Not planned here**: needs sample AAF and OMF files from other applications to build and test against, which the owner is sourcing; DAWproject covers session exchange with Studio Pro and Cubase meanwhile |
| DAWproject | ○ | ● | ○ | ● | **Works**, both ways |

## I. Surround and immersive

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Surround tracks, busses and bounce | ● | ● | ● | ● | **Works** up to 16 channels, correct channel order |
| Fold-down to stereo | ● | ● | ● | ● | **Works**: fixed ITU gains |
| Dolby Atmos objects with a built-in renderer | ● | ● | ● | ● | **Not planned here** (the owner's call); the object switch stays a label |
| Binaural monitoring | ● | ● | ● | ● | **Not planned here**: needs a measured head-related impulse response set, content the owner sources |
| ADM BWF export | ● | ● | ● | ● | **Not planned here**: ADM BWF is the Atmos master's container and carries objects; with Atmos objects and a renderer not planned it has nothing to carry beyond a bed, which a WAV with a speaker mask already does |
| Surround pan automation | ● | ● | ● | ● | **Works**: see F, What can be automated; the centre % and LFE level are not automatable |
| Ambisonics | ● | ○ | ○ | ● | **Partial**: tracks and busses to 3rd order; the panner encodes a source into every order present (SN3D, ACN); decoding to speakers is first order. Left: a decoder of the highest order the layout's speakers support (max-rE weighted), which changes how existing Ambisonic busses sound on 7.1.4 and 9.1.6; orders 4 to 7 need more than 16 channels per strip |
| Correct loudness for 7.1 and wider | ● | ● | ● | ● | **Works**: every layout weighted by its speakers |

Immersive delivery is gap G-12 in [`gaps.md`](../gaps.md). Since Studio One 6.5 all four ship an
Atmos renderer, so this has moved from a post-production extra to common ground.

## J. Mixer state and window

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Undo for mixer moves | ◐ | ● | ● | ● | **Works**: every move, one step per gesture |
| Track presets | ● | ● | ● | ● | **Works** |
| Snapshots or scenes of the whole mix | ○ | ● | ◐ | ● | **Works**: Window › Mix Snapshots stores every strip under a name and recalls it whole or by section (faders, pan, mutes, sends, inserts, routing, delay), on every track or the selected ones (`mix.snapshot_store`, `mix.snapshot_recall`), one undo step each; saved with the session |
| Copy one channel's settings onto others | ● | ● | ● | ● | **Works**: Track › Copy Settings to Selected (inserts and sends, inserts, sends, fader with pan and mutes, or everything but routing; `mix.copy_strip`), one undo step; routing is never copied (it could feed a track into itself); also duplicate a track or apply a preset |
| Import mixer settings from another session onto existing tracks | ● | ◐ | ● | ● | **Works** (2026-11-18, fb7c233): File › Import › Session Data… brings whole tracks as new ones, or (its "Onto tracks with the same names" choice, `file.import_session_data` `onto: true`) lands each of the other session's tracks on this session's track of the same name (case ignored), bringing only the parts ticked: inserts, sends, fader and trim, pan, mute and solo, track delay, automation (`sections`; inserts, sends, fader and pan at first), as Copy Settings to Selected does within a session, in one undo step. Sends go to this session's busses of the same names (made when missing); routing never comes; the clips and audio never come this way; a track with no namesake is listed as not matched. Automation replaces the track's lanes; an insert's lanes come only with the inserts (their modules are the other session's), and the instrument's stay. When importing whole tracks, groups, VCA and folder assignments are still dropped |
| Narrow strips, show and hide sections | ● | ● | ● | ● | **Works** |
| Module graphs and gain reduction on the strip | ◐ | ● | ● | ● | **Works**: live graphs on every built-in module tile |
| Find and filter tracks in the mixer | ● | ● | ● | ● | **Works**: a Find field above the Mix window's strips keeps those whose name holds the text (this window's view: not saved, not undone) |
| Spill a folder, VCA or bus | ● | ◐ | ○ | ◐ | **Works**: Spill in a VCA's, folder's or aux's strip menu shows only it and what it controls or sums (a VCA's members, nested VCAs' too; a folder's tracks at every depth, closed or not; what feeds an aux's bus by output or send), Unspill above the strips puts the rest back |
| Several mixer windows | ○ | ● | ● | ● | **Works** (2026-11-18, 2f658cd): Window › New Mix Window… opens up to three more Mix windows (`window.new_mix`), each in its own native window ("Mix 2", "Mix 3", …), with its own find filter and spill (kept per window) and its own shown sections (its Sections menu; it starts with the first window's). The first Mix window, in the main window or Mix in Separate Window, keeps View › Mix Window Views. They are views only: not saved, closed with the app. Shared by all: strip width and the docked drawers' layout |

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
| Automation playback: breakpoints honoured inside a block, sends, mute and plug-in values ramped, all of it delay-compensated, identical in a bounce | 14–22 | Closes "interior breakpoints" in `gaps.md` G-04. **Done for the fader section** (volume, trim, VCA, mute, pan, sends) and for hosted CLAP and VST3 plug-ins (timed parameter events, 2026-11-17, to the design below); plug-in values are delay-compensated; the built-in plug-ins still change once per block until each takes `set_param_at` |
| Automation writing: controls follow automation, you hear what you move, written at the heard position, return time on release, thin after a pass | 10–16 | **Thin after a pass, faders and pan knobs following, writing at the heard position, a fixed return time and hearing the moved control live (2026-11-18) done** |
| Automate plug-in bypass, instrument parameters and surround pan; make the per-type enables real | 6–10 | **Per-type enables, bypass lanes and instrument lanes done** (2026-11-17; instrument lanes by the MIDI writing work); surround pan lanes done too |
| Key inputs: on the stock compressor, gate and de-esser, and sidechain busses for CLAP, VST3 and AU | 12–20 | Part of G-13. **CLAP and VST3 keys, the generic routing and Comp's detector done**; AU key buses remain (macOS); a key taken from an aux by an ordinary track is done (built from the design below) |
| Controller mapping (MIDI learn) for faders and plug-in parameters | 8–12 | **Done** (section 6, K): faders, pan, send levels and chain knobs; Mackie Control and HUI surfaces too |
| Metering: pre-fader option, RMS and scale choice, a loudness and true-peak meter on the master, output meter without the click | 8–12 | **Done**: RMS, VU, K and PPM scales, and Window › Loudness Meter with the loudness range |
| Pan law choice; balance and width for stereo; per-track delay | 6–10 | **Done**, width and balance automatable too |
| Sends to tracks and hardware outputs, a return made with a new send, default send level, loop refusal | 6–10 | Shared with section 2's cue mixes. **Done**, with pan following the main pan and pre-fader sends that can ignore mute |
| Bounce dialog: source, tail, sample rate, dither and mono options, metadata, add to session; stems as a background job with a choice of tracks and busses | 12–18 | **Done** (2026-11-17); tags in FLAC, AIFF and Opus remain |
| Real-time bounce | 4–6 | **Done** (2026-11-17) |
| Group attributes, suspend all, VCA assignment in the mixer, nested VCAs | 6–10 | **Done** |
| Atmos: objects, a renderer of our own, binaural monitoring, ADM BWF | 50–80 | Is G-12; listed here because all four now have it, sized there |

**3. What most have (≈ 40–65 h).** Mixer snapshots (Studio Pro, Cubase), 8–12 (**done**); a listen bus for
AFL and PFL (all but Logic), 6–10 (**done**, with the control room); post-fader inserts and plug-in deactivation (**done**), 5–8; hardware
inserts with a latency ping, 6–10 (**done**; hearing it needs hardware); multi-mono processing, 6–10 (**done**); write-on-stop and
preview (Pro Tools, Cubase), 5–8; folders without an audio path, mixer search and spill, 5–8.

### Design: automation inside a clip (round three, built 2026-11-17 and 2026-11-18)

Logic's region automation and Studio Pro's part envelopes, for audio and MIDI clips.

- **Stored on the clip.** `Clip::automation: Vec<AutomationLane>`, points in samples from the
  clip's start (moved, trimmed, copied and pasted with the clip; a copy is independent, as loops
  are by owner decision; a shared clip's copies share it as they share notes). Saved only when
  not empty, so existing sessions do not change.
- **How the two combine.** While a clip with a lane for a parameter plays (its range on the
  timeline), the clip's lane is heard: a dB lane (volume, trim, send level) adds to the track's
  lane, any other lane replaces it there. Outside the clip the track's lane plays as now; at the
  clip's edges the value steps (an automatic 64-sample ramp, as mute ramps, avoids a click).
- **Played from a flattened lane.** The mixer is not taught about clips: after each edit that
  touches a clip's automation or a lane it overlays, the engine computes the track's *played*
  lanes (`Track::played`, not saved): the track lane with each clip's points laid over its range
  as above. `Track::played_lane` (already the mixer's only way to read a lane, with the Preview
  suspension) returns the flattened lane when there is one. A bounce, a freeze and playback then
  hear the same thing with no new code on the audio thread.
- **Editing.** Commands `clip.automation_point`, `clip.automation_clear` and two conversions,
  as Logic has: *Move Track Automation into Clip* (the track lane's points under the clip move
  into the clip, as offsets for dB lanes) and *Move Clip Automation to Track*. The clip shows its
  lanes as a line over its waveform (the clip-gain line's drawing); points dragged there.
- **Tests.** A clip lane replaces the track's pan over its range only; a dB lane adds; moving
  the clip moves its automation; the flattened lane is identical in playback and bounce.
- **Size.** 10–16 h, most of it the clip-lane drawing and the editing commands.

### Design: real-time bounce (round three, built 2026-11-17)

- **The switch.** "In real time" in the Bounce dialog (`bounce` `realtime: true`). Only the main
  mix or a bus as the source; stems and Add to Session as for an offline bounce.
- **Playing.** The UI thread locates to the range's start (pre-roll none) and plays to its end
  plus the tail through the live player, so hardware inserts and sends to outboard gear are
  heard and printed as they sound. Editing is locked for the pass (the transport bar shows
  "Bouncing"); Stop cancels.
- **Capturing.** The record taps (`playback::record`) gain a tap on the main mix (or the bus)
  after the master's chain, written by the audio thread into the capture ring each callback
  exactly as the internal-recording taps write busses; the take writer on the disk thread writes
  a float WAV. Positions come from the callback's frame counter, so the file starts at the range's
  first sample whatever the device latency (the mix's own delay compensation is subtracted, as for
  an offline bounce).
- **After.** The float file is fed through the existing bounce writer (rate conversion, dither,
  formats, tags, normalize), so every option of the offline bounce applies unchanged.
- **As built.** The capture is a print tap of its own (`playback::print::PrintTap`, a lock-free
  ring the audio thread fills after `MixEngine::render` and the UI drains each frame) rather than
  a record tap, and the print is held in memory rather than written to a float file
  (`Bounce::printed`): the bounce writer reads it as it reads a render. The click is off while a
  print is attached and skip ranges play through; Stop is Cancel in the progress window.
- **Tests.** With the silent clock (no device) a real-time bounce of the demo is sample-identical
  to the offline bounce. What needs hardware: outboard gear on a hardware insert or send.
- **Size.** 4–6 h.

### Design: multi-mono processing (round three, linked mode built 2026-11-17, unlinked 2026-11-18)

- **A slot of instances.** An insert whose plug-in is mono-only on a wider track (or any insert
  with "Multi-mono" chosen) holds one mono instance per channel: `PluginSlot::plugin` becomes the
  first instance and `PluginSlot::more: Vec<Box<dyn Plugin>>` the rest, made off the audio thread
  by `BuiltinPrep` and the hosted-instance preparation like any instance.
- **Process.** Each instance is fed its channel and writes it (the strip's buffers are already
  per channel, so this is a slice per instance, no copy); latency is the instances' maximum, the
  others delayed to it in the slot's bypass line.
- **Linked or not.** `Insert::multi_mono: Option<MultiMono { linked: bool, params: Vec<..> }>`:
  linked (the default), every instance takes the insert's parameters and its automation; unlinked,
  each channel has its own parameter set (as built: `c<n>:<param>` ids in `Insert::params`) and its own lanes
  (`plugin:m<id>:c<n>:<param>`). The drawer and the plug-in window get a channel picker and a Link
  button, as Pro Tools has.
- **State.** A hosted plug-in's state is saved per instance (built round five: packed in the insert's state while unlinked).
- **Tests.** A linked multi-mono EQ on a stereo track sounds as the stereo instance on identical
  material; unlinked with different gains, each channel takes its own.
- **Size.** 6–10 h.

### Design: hardware inserts with a latency ping (round three, built 2026-11-17)

- **The module.** A built-in insert "Hardware Insert" (`hw_insert`) with an output pair and an
  input pair (Setup › I/O names). The mixer special-cases it in the chain: it writes its input
  into that hardware output's buffer (`MixEngine::hw`, already summed for sends to outputs) and
  reads its return from the device input the mix has (`MixEngine::input`, as tracks hear inputs),
  so the return is the previous callbacks' send by exactly the device round trip.
- **Latency.** The module reports a fixed latency: the measured round trip (a stored parameter,
  samples at the session rate), so delay compensation lines the return up with everything else.
  Without a measurement it uses the round-trip estimate the record offset uses
  (`measure::estimate`).
- **The ping.** "Ping" in the module's drawer runs the existing loopback measurement
  (`Player::measure_latency`, which writes a click into one device output and records one input in
  the same callbacks; `measure::detect` finds the onset) on the insert's own output and input
  and stores the result. The onset detection is already tested in code on synthetic loopback
  recordings (`playback::measure` tests); the new part is choosing the channels from the insert,
  with a test that feeds the insert's output back to its input in code and checks the stored
  round trip.
- **Offline.** A bounce cannot play through outboard gear: offline, the insert passes its input
  through (bypassed, with its latency kept) and the Bounce dialog warns; the real-time bounce
  (above) prints it.
- **What needs hardware.** Everything after the code-level loopback: an interface with a cable
  (or outboard unit) from the insert's output to its input, to hear the return and check the
  ping lines it up.
- **Size.** 6–10 h.

### Design: sample-accurate plug-in automation (built 2026-11-17, first step)

The fader section reads its lanes inside the block; a plug-in's parameters were set once per
block (`apply_params` before `Plugin::process`). The built-in plug-in interface had no way to take
a change part-way through a block.

- **Interface.** `Plugin::set_param_at(offset, id, value) -> bool`, a default method that returns
  false: a plug-in that takes a timed change returns true and applies it `offset` frames into the
  next `process`. Every existing plug-in keeps the default, so nothing it does changes.
- **Host.** After the block-start values (`apply_params`), `apply_timed` looks at each laned
  parameter whose lane has a point inside the block (or ramps across it) and sends its value every
  64 samples where it moves (the sends' and pans' grid), until the plug-in declines. A parameter
  sent so is resent at the next block's start, so the plug-in and the host's record agree again.
  Read at the same delay-compensated position as the block-start values; no allocation.
- **Hosted plug-ins.** CLAP queues a timed `Param` event (its queue is time-ordered); VST3 adds a
  point at the offset to the block's parameter queue (`IParameterChanges` already takes them).
- **Built-ins, next step.** A built-in opts in by overriding `set_param_at`: it keeps the timed
  changes (a fixed-size list) and splits its `process` at them, or ramps its own smoothed value
  towards each. Left for each effect's own work: the change is per plug-in.
- **Tests.** `a_ramp_inside_a_block_reaches_a_plugin_as_timed_changes` (`mix` lib): a ramp inside
  a block reaches a plug-in that takes timed changes as points every 64 samples; one that declines
  keeps its once-a-block value; a still block sends nothing timed.

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
| 2026-11-18 | minor | sends' levels read sample by sample and pans glide between the 64-sample grid's points, so a bending lane no longer steps |
| 2026-11-18 | minor | several Mix windows, each with its own find, spill and sections |
| 2026-11-18 | minor | Import Session Data onto existing tracks of the same name, choosing the parts that come |
| 2026-11-18 | minor | A/B compare for hosted plug-ins from their state |
| 2026-11-18 | minor | multi-mono unlinked: a parameter set and lanes per channel, a channel picker in the plug-in window |
| 2026-11-18 | minor | a clip's own automation points are added, dragged and deleted by mouse in the Edit window |
| 2026-11-18 | minor | a bounce can take a hardware output, with its latency |
| 2026-11-18 | minor | an inactive track frees its plug-ins and makes them again from their stored state when active |
| 2026-11-18 | minor | hearing the control being moved: its lane is held out of playback while a pass writes it |
| 2026-11-18 | minor | a clip's own automation is drawn over it in the Edit window |
| 2026-11-17 | minor | a Hardware Insert not pinged says "not measured" on its slot |
| 2026-11-17 | minor | linked multi-mono processing |
| 2026-11-17 | minor | automation inside clips (moved in and out of the track's lanes; the clip-lane drawing is left) |
| 2026-11-17 | minor | hardware inserts with a ping for their round trip |
| 2026-11-17 | minor | real-time bounce (Bounce Mix › In real time) |
| 2026-11-17 | minor | designs for automation inside a clip, real-time bounce, multi-mono processing and hardware inserts |
| 2026-11-17 | minor | several outputs from one track (additional outputs) |
| 2026-11-17 | minor | automation Preview and Punch Preview |
| 2026-11-17 | minor | surround pan lanes; pencil shapes; basic folders; a stop lets the mix ring out with its meters; the wet/dry Mix of an insert is automatable (section 5); what is left stated on every open row |
| 2026-11-17 | minor | built-in channel processing and AAF/OMF marked not planned here; the Ambisonics rows corrected (3rd-order tracks and encoding, orders above refused) |
| 2026-11-17 | minor | instrument parameters were automated in the MIDI writing work; the suggested order brought up to date |
| 2026-11-17 | minor | automation on tick-timebase tracks follows tempo changes |
| 2026-11-17 | minor | copy a strip's settings onto the selected tracks |
| 2026-11-17 | minor | send controls and built-in module controls follow their automation |
| 2026-11-17 | minor | a PPM scale and the loudness range |
| 2026-11-17 | minor | hosted plug-ins' automation is sample-accurate: timed parameter changes through a new, default-off plug-in method |
| 2026-11-17 | minor | plug-in bypass is automatable |
| 2026-11-17 | minor | linked and mirrored pans; the surround panner's centre % and LFE level get controls |
| 2026-11-17 | minor | find and spill in the Mix window |
| 2026-11-17 | minor | the Touch return time is set in the Automation window; Write on Stop; Show Last Touched |
| 2026-11-17 | minor | Bounce Mix converts the rate, shapes its dither, writes mono or split files and several formats in one pass, tags its files and adds them to the session; stems of chosen tracks and busses |
| 2026-11-17 | minor | VST3 plug-ins take a key on their auxiliary input |
| 2026-11-17 | minor | momentary solo |
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
