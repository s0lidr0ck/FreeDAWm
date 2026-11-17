# Four-DAW parity, section 8: what only one of them has

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-17 · **Change:** minor (tier 4) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

> Pro Tools, Studio Pro / Studio One, Logic Pro and Cubase are trademarks of their owners (Avid,
> Fender/PreSonus, Apple, Steinberg). Other product names below (Dolby Atmos, Melodyne, Splice,
> Moises and the instruments the Band can drive) belong to their owners. FreeDAWm is not affiliated
> with or endorsed by any of them. See [NOTICE](../../NOTICE).

The last section ([1: playing audio](01-playback-audio.md),
[2: recording audio](02-recording-audio.md), [3: mixing](03-mixing.md),
[4: editing audio](04-editing-audio.md), [5: built-in effects](05-built-in-effects.md),
[6: recording MIDI](06-midi-recording.md), [7: writing MIDI](07-midi-writing.md)).

**Scope of this section:** the features each of the four is known for, that the others lack or
do differently, and whether FreeDAWm has an answer; then what FreeDAWm has that none of the four
does. Sections 1–7 compared what the four share. This one compares what sets them apart, so there
is no "all four have it" list: the question for each row is whether it is worth having.

## How this was measured

- **FreeDAWm:** read from the code in the main working folder (0.9.11 plus the part of the work
  on sections 1–7 that had landed there), not from the docs. A feature that another agent has
  since finished on the branch may be marked lower here than it now is; the earlier sections' own
  rows are the record for those. Re-read by hand: higher-order Ambisonics encodes first order
  only, the object/bed switch is read only by drawing code, timing alignment in Fit to Lead was
  refused (since built), nothing outside the video crate calls its encoder.
- **The four DAWs:** from their manuals and from release coverage of the versions above (listed
  under Sources). "Signature" is a judgement: a feature is listed under the product most
  identified with it, even where another has since added something similar.

**Legend.** FreeDAWm: **Works** · **Partial** · **Flag only** (a switch, command or menu item
exists and is stored, but nothing acts on it) · **Missing**.

## Summary

Of 60 signature features checked, FreeDAWm has a working answer to about 12, a partial one to
about 28, and none to about 20.

- **Where it answers well:** virtual players (the Band), stem separation, tempo detection and
  conforming, vocal pitch editing with harmony, clip effects, a sampler track, amp models,
  summing folders, and remote recording over the internet.
- **Where it has nothing:** a clip launcher (Pro Tools Sketch, Studio Pro Launcher, Logic Live
  Loops: three of the four now have one), a mastering or release page, a live-show page,
  Dolby Atmos objects and renderer, AAF/OMF exchange, modulators, articulation maps, a loop
  library, ARA. (A control room with cue mixes, mixer snapshots and Mackie/HUI surfaces have
  since been built in sections 3 and 6.)
- **What it has that none of the four does:** an interface for scripts and AI agents that reaches
  every command; a mix analyser that asks an AI model for advice and draws it on the EQ; local
  models for karaoke, drum splitting, text-to-MIDI and whole-song generation; a build-your-own
  room reverb and impulse capture of gear; a resonance suppressor and a masking EQ in the box;
  a browser build; lyric-writing tools with a teleprompter.

Four controls promise a signature feature and do nothing: the Object/Bed switch and the Renderer
window (Atmos), QuickPunch and TrackPunch, and Ambisonics above first order.

## A. Pro Tools

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Hardware DSP mixing with near-zero monitoring latency (HDX, Carbon) | **Not planned here** | Needs one maker's DSP hardware. A software low-latency monitoring path, for audio tracks |
| AAF and OMF exchange with video editors | **Not planned here** | DAWproject both ways instead. Large formats with little public documentation, and no AAF or OMF files from Pro Tools or a video editor here to build and test against (the owner's call whether to source some) |
| Import chosen tracks and settings from another session | **Works** | Import Session Data, with audio and sampler samples |
| Field-recorder workflow: match and expand location recordings by metadata | **Missing** | Broadcast WAV description is read and written; no iXML, no matching |
| Clip effects: EQ and dynamics per clip, live | **Works** | Per-clip EQ, compressor and gain, with copy, paste and render |
| Beat Detective across many tracks, phase kept | **Partial** | Finds hits across selected tracks and cuts all at the same points; conforming is done by Detect Tempo › Straighten; no trigger editing or edit smoothing |
| Sketch: a clip-launcher window, also on a tablet | **Not planned here** | The clip launcher is skipped by the owner. DAWproject import drops launcher clips and says so |
| Dolby Atmos renderer built in: beds, objects, re-renders, ADM; MPEG-H | **Partial** | Channel surround to 9.1.6 with a 3-D panner works. Object/Bed is a mark that says it is one; no renderer, no ADM. Dolby's renderer needs a licence (not planned); a renderer of FreeDAWm's own is [section 3](03-mixing.md)'s work |
| Speech-to-text on dialogue with text search | **Partial** | Lyric transcription with word times, searchable (AI › Find in Transcript…); no dialogue-specific lane |
| Video: several tracks, editing, satellite link, pull up and down | **Partial** | One video track with H.264, ProRes and Motion JPEG playback, timecode and feet+frames; export to a ProRes movie with the mix; no editing |
| Destructive record, QuickPunch, TrackPunch | **Partial** | QuickPunch works (punch in and out on the fly, several punches in one pass: [section 2](02-recording-audio.md)); TrackPunch and the destructive modes stay off the menus, stored and unread (section 2's rows) |
| EUCON control surfaces | **Not planned here** | Needs Avid's licensed SDK. Mackie Control and HUI surfaces and MIDI learn work instead ([section 6](06-midi-recording.md), K) |
| Scripting SDK | **Works** | Every command, from a script, a command line or an agent (section F) |
| Cloud collaboration on one project | **Not planned here** | Needs a hosted service and a second machine to develop at all |
| AudioSuite with handles; commit, freeze, bounce | **Works** | Process keeps up to 1 s of handles each side for processes that keep the length ([section 4](04-editing-audio.md)); freeze (also up to a chosen insert), commit and bounce work |
| Track Pin, batch rename and other large-session aids | **Partial** | Not checked item by item; Pin Track (`track.pin`) is stored and nothing reads it yet; no batch rename |

## B. Studio Pro (Studio One)

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Project page: master an album in the same app, songs update from their mixes, DDP and digital release | **Not planned here** | The mastering page is tabled by the owner (design below). Bounce to a loudness target with a true-peak ceiling |
| Show page: set lists for playing live | **Missing** | |
| Scratch pads: try another arrangement beside the song | **Missing** | Playlists per track are the nearest thing |
| Arranger track: drag sections to reorder the song | **Partial** | Sections exist for the Band; moving one does not move track audio or MIDI |
| Chord track that re-pitches audio as well as MIDI | **Partial** | Chords drive the Band and re-pitch single-line vocals (Fit to Lead, Make Harmony); MIDI clips follow the chord lane, per clip, opt in ([section 7](07-midi-writing.md)); chordal audio does not |
| Mix Engine FX: one console model across every channel of a bus | **Missing** | |
| Splitter and extended chains: parallel and band-split routing inside the inserts, macro knobs | **Not planned here** | Skipped by the owner (rack and macros). Inserts are a straight chain; linked plug-ins share values, which is not a macro |
| Sound variations (articulations for orchestral libraries) | **Missing** | Keyswitches exist only inside the Band's instrument profiles |
| Launcher: clips and scenes tied to the arrangement | **Not planned here** | The clip launcher is skipped by the owner |
| Lyrics track, lyrics in the score and on a display | **Partial** | Lyrics lane and sheet, a teleprompter for the singer, rhymes and word tools; no lyrics in the score |
| Drag and drop for everything | **Partial** | Plug-in to track or to empty space, audio to sampler, files from the system; nothing drags out |
| Global transpose; audio follows tempo by default | **Partial** | No global transpose; tempo-following is a per-clip choice |
| Listen bus with room correction | **Partial** | An AFL/PFL listen bus in the Control Room, and the monitor stage's ear correction from the Hearing Test (on this computer, never in a bounce); no room measurement or correction |
| Amp and pedal models from a guitar-amp maker | **Works** (unbranded) | Seven amps solved from schematics, cabinets, pedal circuits |
| Tablet remote app | **Partial** | A phone web page on the local network: transport, record, arm, markers; no mixing |
| Built-in assistant, stem separation and vocal tuning (8.1), MIDI 2.0 (8.2) | **Partial** | Stem separation and vocal tuning work; assistant: see Scout (section F); no MIDI 2.0 |

## C. Logic Pro

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Session Players: drummer, bass and keyboard players that follow chords | **Works** | The Band: six players chosen from 152 styles of player, following sections and chords, printable to MIDI. Sounds come from the track's instrument, and FreeDAWm ships few |
| Live Loops grid | **Not planned here** | The clip launcher is skipped by the owner |
| Smart Tempo: record without a click, then fit project or audio | **Works** | Detect Tempo (constant, follow, smooth) writes the tempo map; Straighten conforms audio; no free-tempo record mode as such |
| Flex Time and Flex Pitch on the track | **Partial** | Pitch in a separate vocal editor; time by clip stretch and by warp markers placed by hand or by Straighten, heard at once (`warp.*`); no pitch editing on the track itself |
| Stem Splitter, now down to single drums | **Works** | Four or six stems, lead and backing vocals, and drums into kick, snare, toms, hat, ride, crash. Models are downloaded |
| Mastering Assistant | **Partial** | Scout measures the mix and returns advice, and applies it a step at a time when asked; no single-press mastering chain |
| Drum Machine Designer, Alchemy, Sculpture and the rest of the instrument set | **Partial** | A capable drum machine and a one-sample sampler; nothing like the synths ([section 7](07-midi-writing.md#l-instruments-in-the-box)) |
| Smart Controls; tablet remote with touch instruments | **Partial** | A few key knobs per insert on the mixer; transport-only phone remote |
| Track Stacks and patches | **Works** | Routing folders sum their tracks; track and chain presets carry their samples |
| Scripter and the Environment | **Missing** | Scripting is from outside the app only |
| Spatial audio with binaural monitoring and head tracking | **Partial** | Surround panning; no binaural path |
| A very large sound and loop library; loops follow tempo and key; one browser for everything | **Not planned here** | Content and licences, the owner's call. The browser lists plug-ins and presets; shipped content is about 20 MB |
| Round trip with the tablet edition and with MainStage | **Not planned here** | Apple's own apps and formats |
| Chord ID from audio | **Works** | Detect Chords writes the chord lane |
| Flashback capture | **Partial** | MIDI: Capture MIDI makes a take of what was just played ([section 6](06-midi-recording.md#g-punch-count-in-and-capture)); audio played before Record is not kept ([section 2](02-recording-audio.md)) |
| Step Sequencer with note and automation rows; articulation sets | **Partial** | Drum step sequencer; no automation rows, no articulations |
| Assistant that operates the app from a prompt (13, by subscription) | **Partial** | No assistant inside the app; an outside agent can operate every command (section F) |

## D. Cubase

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Expression Maps and Note Expression | **Missing** | |
| VariAudio: pitch and formant editing, harmonies from the chord track | **Works** | Note-based tuning with formant-preserving resynthesis; Make Harmony follows the chords; a view of every vocal track with clashes marked |
| Chord Pads and Chord Assistant | **Partial** | A chord picker offering the key's triads and sevenths; no pads, no suggestions |
| Control Room: cue mixes, talkback, monitor sets | **Works** | Window › Control Room: monitor sets with level, dim and mono, AFL/PFL listen, cue mixes from pre-fader sends, talkback ([section 3](03-mixing.md), ec5efc3) |
| Mixer undo history and mixer snapshots | **Works** | Mixer moves are ordinary undo steps; Window › Mix Snapshots stores and recalls the whole mix or parts of it ([section 3](03-mixing.md)) |
| Logical Editor, Project Logical Editor, macros | **Missing** | |
| MIDI Remote: scripted controller support with a mapping assistant | **Partial** | MIDI learn for faders, pan, sends and plug-in knobs, Mackie Control and HUI ([section 6](06-midi-recording.md), K); no scripts for other controllers |
| Sampler Track with slicing | **Works** | Drop audio on an instrument track or send a selection to the sampler; slices by hits, beats or equal parts |
| Audio Alignment of stacked takes | **Works** | Vocal Studio's Align timing (Fit to Lead's timing half): each note of a double moves its start and end onto the lead's, with a strength and a largest shift; pitch kept (aee379b) |
| Hitpoints, AudioWarp, Time Warp | **Partial** | Detection, Straighten and Conform, and warp markers placed by hand; no Time Warp of the tempo map by dragging |
| Direct Offline Processing: an editable offline chain per clip | **Partial** | One-shot Process; Clean Vocal keeps the original with an amount you can change later |
| Modulators for any parameter | **Missing** | |
| Track Versions, arranger chains, Render in Place | **Partial** | Playlists; commit and bounce; no arranger chains |
| Remote recording over the internet (VST Connect) | **Partial** | Works, by a different design (section F), with talkback (4d3e275); no video |
| ARA 2 and spectral editing | **Not planned here** | ARA needs its SDK, which is not in the tree; spectral editing is [section 4](04-editing-audio.md)'s. A view flag exists and is unread |
| Score editor with engraving; drum and pattern editors, now melodic | **Partial** | A read-only score with print and MusicXML export; drum step sequencer only |
| Video export; loudness track and normalising on export; metering suite | **Partial** | Bounce to a loudness target with a true-peak ceiling; export to a ProRes movie; no loudness track; meters are strong (BS.1770 with true peak, LevelUp, Scout) |
| Vocal synthesis (Omnivocal, beta) | **Not planned here** | Needs a singing-voice model, and no third-party models are added here. Song generation produces finished audio, not an editable sung part |

## E. Where three or four now agree

These started as one product's signature and have spread. They are the gaps most likely to be
noticed, and the ones sections 1–7 did not already list:

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Clip launcher | ● | ● | ● | ○ | **Not planned here** (skipped by the owner) |
| Stem separation | ● | ● | ● | ● | **Works** |
| Sound and loop service or library in the browser (Splice or the maker's own) | ● | ● | ● | ● | **Not planned here** (content, the owner's call) |
| Dolby Atmos mixing with a renderer | ● | ● | ● | ● | **Partial** (see A) |
| Arranger sections that move the song | ○ | ● | ● | ● | **Partial** (Band only) |
| Chord track that existing clips follow | ◐ | ● | ● | ● | **Partial** (MIDI clips, opt in; not audio) |
| ARA 2 | ● | ● | ● | ● | **Not planned here** (no SDK in the tree) |
| An assistant inside the app | ○ | ● | ● | ○ | **Partial** (Scout advises on the mix only) |
| Tablet or phone remote | ● | ● | ● | ● | **Partial** |

## F. What FreeDAWm has that none of the four does

Each was checked in the code. "Works" means wired end to end; none was listened to or timed.

| Feature | Status | What it is, and its limits |
|---|---|---|
| Every command open to scripts and AI agents | **Works** | About 330 commands with documented parameters. A command-line tool runs them on a session without the window; a running app accepts them on a local port; an MCP server lets an AI agent list commands, run them, look at the session, take screenshots and click. Any menu item can be invoked by its path |
| Remote recording with the take landing in place | **Works** | A second computer follows the transport, records locally and sends the take (lossless audio, or MIDI). The performer hears the live mix; clocks are compared during the take so the audio lands where it was played. Works on a local network or across the internet (encrypted, by one-time invite, no account). Takes survive a dropped connection or a crash. On a local network PCs pair and connect over an encrypted link bound to their keys (4d3e275); the phone's page stays on the plain link. Talkback: the control room's talk key mixes its input into the stream the performer hears, the music dimmed, while stopped too; never recorded (4d3e275). No video. A paired computer can run any command; the Network window says so and which link each uses |
| Scout: measured mix advice from an AI model | **Works** | Captures loudness, true peak, spectrum, stereo and transient measures of a mix or a channel, sends them with the channel's settings to Claude using the user's own key, and draws the returned EQ suggestions on the EQ as guides. It changes nothing by itself; each suggestion can be applied on request, one undo step each. Reference profiles can be built from other recordings, per stem |
| Local models beyond stems | **Works** | Lead from backing vocals; drums into six parts; audio to MIDI by four models (general, piano with pedal, drums, voice) with a live preview; MIDI from a text description; a whole song from a prompt. All run on the user's machine after a download; each model's licence is shown, and non-commercial ones ask for confirmation |
| Mend: vocal de-noise and de-reverb on a live insert | **Works** | The same model as Clean Vocal, running in the chain with about 2.9 s latency or a lighter live mode. Desktop only |
| MIDI Cleanup against key and chords | **Works** | Corrects a transcribed part without changing starts, velocities or pitches of kept notes; switchable until committed |
| Vocal Studio: every vocal on one pitch view | **Works** | Marks clashes and unisons across tracks, moves clashing notes to fit the lead, lines up a double's note starts and ends with the lead's, writes a harmony |
| ChamberMade; Imprint and Vault | **Works** | A reverb from a room you draw (size, materials, objects, speaker and microphone); capture of an impulse response from any plug-in, outboard unit or room, then convolution with it |
| Rez, Manners, Lull's guard, Shale | **Works** | A resonance suppressor; an EQ that reduces masking between bands by itself; a gate that listens to the source it should ignore; a drum gate that learns bleed, links across the kit's microphones and can send its hits as MIDI |
| LevelUp | **Works** | Loudness-matched before-and-after listening by a linked pair of inserts, with reference tracks and a preview of what streaming services do to the level |
| Lyric tools | **Works** | Rhymes, synonyms and stress checks offline; alternative lines; a teleprompter; timed lyrics from a recording |
| Browser build | **Not planned here** (the owner: no further work) | The same app in a web page: playback, editing, built-in effects, save and export as downloads; the session is kept in the browser between visits, said on the page, with Export and Clear (270aee2). Recording dropped by the owner (no one will use the browser build); no hardware MIDI, third-party plug-ins, models or networking |
| Its own video decoders | **Works** | H.264, ProRes and Motion JPEG without system codecs; its own ProRes encoder writes the movie export |
| Open source, five platforms | **Works** | MIT or Apache-2.0 (the Windows build with ASIO is GPLv3). Linux, macOS, Windows, FreeBSD and the web. English only; little work on screen readers |
| Take recovery and safety by tooling | **Works** | A running take is recovered after a crash; ten backups per session; the build refuses code that can panic outside tests. A hosted plug-in runs inside the app and can bring it down, unless it is a CLAP plug-in and Preferences › Mixing › Run CLAP plug-ins in their own process is on (off by default; see the design notes) |

## Things that promise more than they do

| # | What | Where | Status |
|---|---|---|---|
| 1 | Object/Bed switch on the mixer strip and the Renderer window: the switch is stored and drawn; no audio path reads it, and the Renderer only draws speakers and dots | `engine/src/cmd/track_more.rs:21-40`; `ui-egui/src/mix_window.rs:408-414`; `ui-egui/src/extra_windows.rs:86-145` | **Labelled** (943143a): the switch's hover, its command's description and the Renderer window say that objects are not rendered yet and play through their panners into the bed. Rendering objects is the mixing section's work ([section 3](03-mixing.md)) |
| 2 | Track widths of 2nd- to 7th-order Ambisonics can be chosen; the panner fills the first four channels and leaves the rest silent | `dsp/src/pan.rs:268-279` | **Done** (a2e9845): the panner encodes every order the track carries (ACN, SN3D, to 7th order in the DSP), so 2nd and 3rd order are filled; the mixer carries at most 16 channels, so 4th to 7th order are no longer offered and are refused by the commands that make tracks, busses and outputs. The decoder to speakers is still first order |
| 3 | QuickPunch, TrackPunch, Destructive Record and Destructive Punch have keys or commands and store a mode nothing reads; Prepare DPE Tracks consolidates for a punch that never happens | `engine/src/cmd/options.rs:22, 58-59, 125-130`; `engine/src/cmd/setup_more.rs:22, 130` | **Another section's**: punch modes are the recording work in [section 2](02-recording-audio.md); QuickPunch landed there (4b98b7f) |
| 4 | Stored and unread: transcription lane and settings, scene and take in clip names, ARA note overlay, peripherals, video sync offset, Ableton Link in the transport, calibration mode, auto-spot | `engine/src/cmd/view_more.rs:145, 154`; `engine/src/cmd/setup_more.rs:31-53` | **Done; claim partly wrong** (8fac425): these were already off the menu bar (the 2026-10 menus clean-up); agents and scripts still reach them, and their descriptions now end "stored only: nothing in FreeDAWm reads this setting yet". The video sync offset is not one of them: the Video track and window read it (it has no window of its own) |
| 5 | The web build has no way to record and no place that says so; MIDI windows are shown there with nothing behind them | `apps/soundcraft-web`; `ui-egui/src/lib.rs:996-999` | **Done for recording** (b5ef0ca): in the browser build Record says it cannot record there, on the button and in the status line. The MIDI windows are the MIDI recording section's ([section 6](06-midi-recording.md), row 23), which left them out of the web build's menus (352090a) |
| 6 | On a local network, a paired computer can run any command over an unencrypted link. The pairing code is four digits with attempt limits. Fine for a studio network; worth saying in the window, and worth offering the encrypted path locally too | `net/src/hub.rs:633, 794, 1026`; `net/src/pairing.rs:1-30` | **Done** (7ed1bfd): the Network window and the pairing prompt say that a paired device can do everything the user can. The encrypted local link is **done** too (4d3e275, below) |
| 7 | Video muxer and ProRes encoder exist in the video crate and nothing calls them | `video/src` | **Done** (3b781d6): Bounce to Movie uses both |

## Suggested order of work

Estimates are agent-hours for one implementer with tests. These are larger and less certain than
in sections 1–7, because most are new features, not repairs.

**1. Make the controls honest (≈ 6–10 h).** Rows 1–5 above: remove or grey out what does nothing
(object switch, Renderer, higher Ambisonics orders, the punch modes not being built in section 2,
the web build's MIDI and record controls), or build it where a later row here does. Say what the
local link allows in the Network window (row 6).

**2. Signature features worth taking, best value first (≈ 190–300 h).**

| Work | Est. (h) | Why | Status |
|---|---:|---|---|
| Arranger sections that move everything, and clips that follow the chord track | 14–22 | Sections and chords already exist for the Band; this makes them act on the user's own tracks. Also in [section 7](07-midi-writing.md) | **Section 7's**: MIDI clips follow the chord lane there (per clip, opt in); arranger sections that move clips are still Missing there |
| Bounce to a loudness target; video export | 10–16 | Both have their parts already (loudness meter; muxer and encoder) | **Loudness target done** (ca44ec5): Bounce Mix takes a target in LUFS (integrated, BS.1770) with a true-peak ceiling (default -1 dBTP), measured in a first pass; gain only, so a mix that would pass the ceiling stops short and says by how much. Video export **done** (3b781d6): File › Export › Movie (ProRes)… writes the Video tracks' picture and the main mix as a QuickTime movie, ProRes 422 (proxy, LT, standard or HQ) at the first movie's size and frame rate, with 24-bit PCM stereo |
| Scout builds a starting chain from its advice, with the user's consent per step | 8–12 | Turns advice into Logic's Mastering Assistant, on any channel | **Done** (06244b1): each suggestion of an answer has an Apply button; `scout.plan` says what each would do, `scout.apply` does one, as one undo step. An EQ guide becomes a band on the track's Clean Q (one is added if missing); a module suggestion sets its parameter to the middle of the suggested range when that range is in the parameter's own units, and is refused otherwise. Uses the answer already received: nothing more is sent |
| Timing alignment of stacked vocals | 10–16 | The pitch half is done; the command already has the option | **Done** (aee379b): `vocal.fit_to_lead {align_timing: true}` (the studio bar's Align timing, which leaves pitch alone with `fit_pitch: false`). Each backing note is paired with the lead note it overlaps most; its start and end move `strength` (default 100 %) of the way to the lead's, at most `max_shift_ms` (default 100). Where a note starts out of silence or stops into it the edge is read from the audio (within 25 ms of the tracker's), so edges land closer than the tracker's 5 ms hop. The clip is rendered again with Straighten's pitch-keeping stretch between those points (its edges stay; Remove Warp restores the original), its tuning carried through the warp and re-matched to an analysis run before the command returns: one undo step. Test: a double sung 17 to 41 ms off lands every start and end within 3 ms of the lead's, pitch within 8 cents, a fitted note keeps its fit. Limits: Remove Warp goes back to the original audio without the tuning; Clean Vocal's amount at the time is baked into the render; timed lyrics on the clip are not moved |
| Search the transcript; transcript lane | 6–10 | Whisper output is there; Pro Tools' dialogue workflow | **Search done** (ade867d): `lyrics.search` lists every place a phrase is sung or spoken across the tracks' timed lyrics (case, punctuation and line breaks ignored); AI › Find in Transcript… selects the next one and moves the playhead there. The lane is the lyric strip each track already draws; a separate taller dialogue lane is not built |
| Macro knobs and a splitter in the insert chain | 14–22 | Also in [section 5](05-built-in-effects.md)'s list; count once | **Not planned here**: rack and macros skipped by the owner |
| Modulators for any parameter | 10–16 | Also in section 5's list | **Another section's** ([section 5](05-built-in-effects.md)) |
| Control room: cue mixes from sends, talkback, monitor sets, listen bus | 14–22 | Pairs with remote recording, which has no talkback | **Done** in [section 3](03-mixing.md) (ec5efc3) |
| Mixer snapshots | 6–10 | | **Done** in [section 3](03-mixing.md) (Window › Mix Snapshots) |
| Clip launcher: cells, scenes, record the performance to the timeline | 40–60 | Three of the four have one. The largest item; needs a design first | **Deferred by the owner** (2026-11-16): not to be built for now |
| A mastering page: several songs in order, loudness per song, export a release | 24–36 | Only Studio Pro has it; a strong reason people choose it | **Not planned here**: tabled by the owner (design below, kept) |
| Atmos: objects and beds routed for real, ADM export, a binaural monitor | 30–50 | A renderer of FreeDAWm's own; Dolby's needs a licence. Also in [section 3](03-mixing.md) | **Another section's** ([section 3](03-mixing.md)) |
| ARA 2 hosting | 30–45 | Also in [section 4](04-editing-audio.md) | **Another section's** ([section 4](04-editing-audio.md)) |

**3. Not worth chasing, or not possible.** Hardware DSP mixing and EUCON (tied to one maker's
hardware); cloud collaboration and tablet companions (a service and second app to run); a large
sound library (content and licences, the owner's call); AAF and OMF (large formats with little
public documentation; DAWproject covers the modern case); a live-show page and field-recorder
workflow (specialist; revisit if users ask); vocal synthesis.

**4. Build on what only FreeDAWm has (≈ 40–65 h).** Recording in the browser build, with
sessions kept between visits (12–18): sessions kept **done** (270aee2), recording **dropped by the owner**
(the browser build will not be used; design kept below); talkback and encrypted local links for remote recording
(10–16): **done** (4d3e275, see the design notes below for what was built and what remains); isolate hosted plug-ins in their own process so a crash cannot take the session (16–28,
and the scan helper is a start; **first steps built** (3b77990, and round three): CLAP, a
Preferences option off by default, the helper shipped in every package; see below); a first-run tour (4–6): **done** (a689967), seven cards opened once on the first start (no settings saved yet) and again from Window › More › Tour of FreeDAWm.

**Designs for the larger items** (some since built, as each says). Each was judged too big to finish well in the
same pass as the items above; these are the starting points.

- *Mastering page (24–36 h).* A release file of its own (`.fdrel`, JSON) beside sessions: an
  ordered list of songs, each a session (bounced on demand, so a song follows its mix) or an audio
  file; per song a gain or loudness target (the bounce's two-pass measurement, already there), a
  gap and fades; one shared master chain plus a chain per song, run offline through
  `RangeRender`. A `release.*` command family (add, move, remove, set, render) and a page that
  shows the songs in a row with their integrated loudness, true peak and the album's. Export:
  one file per song plus one continuous file with a cue sheet; DDP later.
- *Sessions kept in the browser build (built, 270aee2).* The origin-private file system (OPFS),
  through a small script in the page (`window.freedawmStore`): `session.json`, `meta.json` and
  `Audio Files/<source>-<fingerprint>.wav`, each audio file written once
  (`io::session_store_parts`), the session 3 s after a change and never while the transport
  rolls, audio before the session that points at it, unused files removed after. The next visit
  opens it (`io::open_session_parts`, the bundle opener's own path) instead of the demo; `?demo`
  and `?empty` still start fresh. A notice at the bottom right says "Kept in this browser only"
  (or why not: no OPFS, a failed write), with Export (a `.fdawmz` download) and Clear. Not run in
  a browser here: the Rust side builds for wasm and its parts are tested natively
  (`io` tests); the script and the page need a browser (Chrome, Edge, Firefox; Safari before 18
  lacks `createWritable`, and the notice then says the session is not kept).
- *Recording in the browser build (dropped by the owner, 2026-11-17: the browser build will not be
  used).* The in-memory take writer it needed is in (c4c24e1, kept). Tried in headless Chromium
  before it was dropped, the browser build did not start: `std::time::Instant` panics in a browser
  and the disk cache makes one when the first session is sent (and its output asked for 32
  channels where a page has 2, so there was no sound). Both were fixed in a patch kept aside with
  the recording wiring, which worked there (a take landed and was kept across a reload). The
  design as written: The parts, in order: (1) the page:
  `getUserMedia({audio: {echoCancellation: false, noiseSuppression: false, autoGainControl:
  false}})` asked on the first Record (the browser asks the person; a refusal or no device is
  said plainly on the Record button and the status line, and Record stays refused, as now), an
  `AudioWorklet` that posts its 128-frame blocks over a `MessagePort` (a `SharedArrayBuffer` ring
  needs cross-origin isolation the host cannot be assumed to give); (2) the player: an external
  input feed (`Player` takes an `InputFeed` the page pushes into, as the desktop's input callback
  does), so monitoring and capture run in the same output callback as on the desktop; (3) the
  recorder: wasm has no threads and no files, so the take writer runs a step a frame from the UI
  thread into memory, and a finished take lands as a pooled source (kept by the store like any
  other); (4) latency from `AudioContext.baseLatency` + `outputLatency` for the output and the
  worklet's block for the input, taken off as the desktop's reported latencies are. Steps 2 and 3
  change `crates/playback/src/record.rs` and the UI's take landing, shared with the desktop
  recorder; they were left for a pass of their own.
- *Encrypted local link (built, 4d3e275; owner-approved UDP port).* The internet path's QUIC
  (TLS 1.3, iroh) on the local network: a UDP socket on the network port's number beside the TCP
  listener, with no relays, no address lookup and no router port mapping, so nothing leaves the
  local network. Each PC's key is the one it already keeps for the internet. The booth adds its
  UDP port and key to the mDNS record it already sends; a remote PC dials that key (only its
  holder can answer) and pairs with the code on the booth's screen, attempt limits as before.
  The booth binds the paired PC to its key (its token is refused on the plain link and from any
  other key); the remote PC keeps the booth's key and later dials only it. Setup › Network:
  Offer the encrypted local link (on by default), Take PCs over the encrypted link only (the
  plain link then serves the phone's page alone; on by default since the owner's decision of
  2026-11-16, a value already saved is kept), and which link each PC is on. *Not built:* the
  password-authenticated exchange over the code (SPAKE2): no such crate is in the dependency
  tree and hand-written key-exchange code was not wanted (a design naming the crate is below, for the owner to approve). Without it, a machine that answers the
  very first contact with its own key (by forging the booth's mDNS record) could still sit in the
  middle of that one pairing, as it can on the plain link today; once paired the keys are pinned.
  PCs paired over the plain link keep working there only while Take PCs over the encrypted link
  only is off (settings saved before the option existed now have it on: such a PC pairs again,
  over the encrypted link). The phone's page stays on plain HTTP (a browser cannot speak this
  link) and is served with the option on.
- *Talkback (built, 4d3e275).* The control room's talkback (its input, level and talk key, F11 or
  the Network window's Talk to the performer, held) is mixed into the copy of the mix the stream
  sends, the music dimmed by the control room's dim (once: the monitor's own dim already counts
  when talkback dims), ramped over 10 ms. While the transport is stopped the stream carries the
  talkback alone, on its own clock, and the performer hears it stopped too; the next roll starts
  a new epoch. It is never in the booth's main mix or a bounce, and the performer's take records
  his own inputs, never the stream.
- *Plug-ins in their own process (16–28 h; first step built, 3b77990; design refined 2026-11-16).*
  *Built in the first step:* the crate `plugin-bridge` with `BridgedPlugin` behind the `Plugin`
  trait; the helper `freedawm-plugin-host` (a binary of that crate; found next to the app or by
  `FREEDAWM_PLUGIN_HOST`); one shared file per instance mapped with `memmap2` and read only as
  atomic words; a JSON-lines pipe for open, prepare, state save and load; a block not back in time
  plays its own input (an effect) or silence (an instrument) in its place, counted, so the latency
  (one block, reported) never moves; offline renders wait for each block (at most 5 s) with no
  added latency; a helper that exits, or falls 5 s behind, silences its plug-in (an effect passes
  its input through, still one block late) and is said once, by name, on the status line and in
  the log. CLAP only, opt-in by `FREEDAWM_PLUGIN_PROCESS=1` (off by default: nothing changes for
  existing sessions). Tests: the exchange with two threads standing in for the processes (in
  order, one block late; a late block passed through and its late output dropped; offline waits;
  hostile headers refused); the real helper serving a built-in plug-in and the CLAP test plug-in
  (output equal to the input times its gain, one block late; latency its own plus one block); the
  helper killed mid-render: every block still returns at once, the plug-in passes audio through,
  the crash is reported by name, later requests fail at once. *Second step (round three):*
  Setup › Preferences › Mixing › Run CLAP plug-ins in their own process (`ui.plugin_process`,
  this computer's, off by default; it applies to plug-ins loaded from then on and when a session
  opens; `FREEDAWM_PLUGIN_PROCESS=1` or `0` overrides it for a run, and the window says so, and
  says when the helper is missing); the helper is built with the workspace and every package
  build (Linux, Flatpak, macOS, FreeBSD, Windows MSI and zip) carries and signs it next to the
  app (a test reads the scripts). With the option on, a CLAP plug-in's description (name,
  parameters, group) is read in a helper of its own too, so one that crashes or hangs as it
  loads only costs that helper: it is not offered, the status line says so once, and it is never
  then loaded in the app's own process (nor is one the helper could not make for an insert). A crashed or hung plug-in's tile says "crashed" (red mark, as a failed module), the status line says Restart brings it back, and Restart Plug-in on its menu (in the chain, the Mix window's inserts and the instrument slot; `ui.plugin_restart`, `mix.module_restart`, not an undo step) makes it again in a new process from the state it last had (read from it whenever the app reads states, or loaded into it; a crashed one answers that) and the parameter values the session holds. A loaded state now also sets the host's copy of the parameters, and only values the host set go to a new exchange (before, a re-prepare sent every default over a loaded state). Not built: an automatic restart, and a state fetched after each edit in the plug-in's own editor (it has none through the bridge yet). *Differs from the design:* the
  unsafe code is the crate's own private `shm` module (the mapping and its view as atomics); the
  helper waits by spinning, yielding, then 200 µs sleeps (no futex or named event). *Not built
  yet:* a per-insert choice,
  the editor window, the plug-in's own
  presets, latency, tail and parameter changes the plug-in makes itself, the transport for the
  plug-in (tempo, position), VST3 and AU. The scan itself already ran in `freedawm-scan`.
  The design as written: Behind an
  option (Setup › Plug-ins › Run plug-ins in their own process, off by default; per insert too),
  CLAP first, VST3 and AU after on the same bridge.
  - *Where it sits.* A new crate `plugin-bridge` with `BridgedPlugin`, which implements
    `soundcraft_dsp::Plugin` (the trait `clap-host`'s `ClapPlugin` implements now), so the mixer,
    automation, presets and Freeze see no difference. The helper is a new binary,
    `freedawm-plugin-host`, next to `freedawm-scan`; it loads one plug-in with `clap-host` as
    the app does now and serves it. `plugin-scan`'s `helper_exe` finds it and its process
    handling (spawn, kill, a timeout) is reused. The unsafe code stays in the FFI crates.
  - *Audio.* One shared-memory file per instance (`memmap2`, already in the dependency tree;
    a file in the temp folder, removed when the slot goes): a header (block size, channels,
    sample rate, a sequence number each way, the overrun count) and two blocks of planar
    `f32` audio in and out, plus a fixed event area (notes, MIDI, parameter changes with their
    sample offsets, up to the existing `EVENT_CAP`). The audio thread writes block *n* and its
    events, bumps its sequence number, and reads block *n − 1*'s output, never waiting:
    `latency()` reports one block more than the plug-in's own, so delay compensation lines it up.
    The helper waits on the sequence number (a futex on Linux, a named event on Windows, a
    short spin then a sleep elsewhere), runs `process`, bumps its own. An output not back in time
    is silence, counted and shown on the insert (an overrun), never a stall. Offline renders
    (bounce, freeze) run the helper synchronously, block by block, with no added latency.
  - *Control.* A pipe (the child's stdin/stdout, JSON lines like the scan helper's) for
    everything off the audio thread: load, activate, parameter info and values, state save and
    restore, latency and tail changes, presets, and the editor window. The editor opens in the
    helper as its own top-level window (a CLAP floating window; embedding across processes is
    platform work for later), which the app raises and closes.
  - *A crash.* A helper that exits or stops answering the pipe for 5 s is dead: its slot passes
    the audio through (an effect) or is silent (an instrument), the insert says "crashed", and
    the session is untouched. Restart (on the insert, or automatically once per minute at most)
    starts a new helper from the state last fetched, which is fetched after every edit gesture
    and before every save, so at most the last gesture is lost.
  - *Tests.* A test plug-in that crashes on a parameter value and one that hangs: the session
    keeps playing, the slot reports it, Restart brings it back with its state; a bridged
    plug-in's output equals the in-process one's delayed by one block; an offline bounce through
    a bridge equals the in-process bounce sample for sample.
  - *First step when picked up.* The shared-memory block exchange and its test (two threads of
    one process standing in for the two processes), then the helper binary, then the option.
- *First pairing by a password-authenticated key exchange (design, 2026-11-17; not built: needs a
  new dependency, the owner to approve).* The weak spot left by the encrypted local link is the
  very first contact: a machine that forges the booth's mDNS record answers with its own key and
  could sit between the two for that one pairing. The fix is to make the four-digit code prove
  the keys. *Crate:* `spake2` (RustCrypto's PAKEs, MIT OR Apache-2.0, pure Rust, no unsafe;
  SPAKE2 over Ed25519). It would bring `curve25519-dalek` 4 beside the 5.0 already in the tree
  (through iroh) and `hkdf`; no C code, nothing that opens a port or sends anything new. A
  hand-written exchange stays ruled out. *Exchange* (in `net/src/pairing.rs`, inside the QUIC
  connection the remote PC already opens to the key in the mDNS record, so nothing new
  listens): the booth is side B, the remote PC side A, identities `freedawm-booth` and
  `freedawm-pc`, password the code shown on the booth's screen. Each sends its SPAKE2 message
  (33 bytes); each derives the shared key `k`; each then sends
  `HMAC-SHA256(HKDF(k, "freedawm pairing confirm"), role || booth key || PC key)` over the two
  QUIC public keys as each side sees them in the TLS session. A side whose check fails drops the
  connection and counts a failed attempt (the attempt limits as now). The code itself is never
  sent. A machine in the middle holds a different TLS key towards each side, so the confirm
  messages cannot match unless it guessed the code, one online guess per attempt (at most the
  attempt limit out of 10 000). Once confirmed, the keys are pinned exactly as now. *Kept as is:*
  the phone's page on the plain link (a browser cannot run it), PCs already paired (their keys are
  pinned), and nothing changes for a PC with an older FreeDAWm: the booth answers its pairing with
  "update FreeDAWm on this computer" (a version field in the pairing hello). *Tests:* two hubs in
  one process pair over loopback with the right code and refuse a wrong one; a third hub relaying
  between them with its own key (the forged-record case) makes both sides fail the confirm and
  count an attempt; a recorded exchange replayed fails; the attempt limit still locks.
- *Editor windows, VST3 and Audio Units through the bridge (design, 2026-11-17).* *Editor:* the
  helper opens the plug-in's editor as a top-level window of its own (CLAP `gui` floating, VST3
  `IPlugView` in a window the helper makes, AU's view in an `NSWindow`), on the helper's main
  thread, which already serves the pipe; the serve loop gains a platform event pump between
  blocks (Win32 `PeekMessage`, Cocoa run loop for a slice, X11/Wayland through the same helper
  the in-process editors use). The pipe carries `editor_open`, `editor_close`, `editor_raise`,
  and the helper sends unrequested lines for parameter changes the editor makes (`param`, with
  gesture begin and end), latency and tail changes, and "editor closed", read by the pipe's reader
  thread into the queue the UI's `idle` drains as it does for in-process editors (`EditorEdit`).
  Embedding the editor in FreeDAWm's own window across processes is platform work for later
  (`XEmbed`/foreign-window parenting, a child `HWND` from another process, no supported way on
  macOS). *VST3:* the helper makes the plug-in with `soundcraft-vst3-host` (its id `vst3:…`)
  exactly as it does CLAP now; the bridge, the exchange and the crash handling do not change;
  the scan's VST3 results already carry the bundle path for `dir`. *AU:* the same with
  `soundcraft-au-host` on macOS; an AU's out-of-process loading by the system (AUv3 extensions)
  is left to the system. *Order:* VST3 first (its test plug-in is in the tree, so it is a
  tested step), then the CLAP editor (needs a desktop session to test), then AU (needs a Mac).

## Sources

Versions as in [section 1](01-playback-audio.md#sources). Release coverage used for this
section:

- Pro Tools 2026.4 and 2026.4.1: [Avid, What's New in Pro Tools 2026.4](https://www.avid.com/resource-center/whats-new-in-pro-tools-20264); [Mixdown, 2026.4.1](https://mixdownmag.com.au/news/pro-tools-2026-4-1-brings-m5-mac-support-to-avids-daw/)
- Fender Studio Pro 8 to 8.2: [Fender newsroom, 8.2](https://www.fender.com/newsroom/273556-fender-studio-pro-8-2-expands-mixing-and-creative-production-tools/); [MusicTech, 8.1](https://musictech.com/news/gear/fender-studio-pro-8-1/); [Synth Anatomy, 8](https://synthanatomy.com/2026/01/fender-studio-pro-the-new-studio-one-daw-with-presonus.html)
- Logic Pro 13: [Apple release notes](https://support.apple.com/109503); [Production Expert](https://www.production-expert.com/production-expert-1/logic-pro-13-splits-drum-stems-into-kit-pieces-and-rebuilds-the-browser); [MusicRadar](https://www.musicradar.com/music-tech/apple-goes-all-in-on-ai-with-logic-pro-13-introducing-an-intelligent-assistant-that-can-control-your-daw)
- Cubase 15: [Sound On Sound review](https://www.soundonsound.com/reviews/steinberg-cubase-pro-15?amp); [Synth Anatomy](https://synthanatomy.com/2025/11/steinberg-cubase-15-daw-receives-a-major-update-with-new-features.html)

Search summaries of these pages were used, not the full pages; details such as which edition
includes a feature should be checked before being quoted.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-17 | minor | Plug-ins in their own process: a crashed plug-in says so on its tile, and Restart Plug-in brings it back with its last state |
| 2026-11-17 | minor | Designs: the first pairing by a password-authenticated key exchange (the crate named, not added), and editor windows, VST3 and AU through the plug-in bridge |
| 2026-11-17 | minor | Plug-ins in their own process: a CLAP plug-in's description is read in a helper too, and one that crashed there is never loaded in the app |
| 2026-11-17 | minor | Plug-ins in their own process: a Preferences option (off by default, the variable overrides it) and the helper in every package |
| 2026-11-17 | minor | Rows reconciled with the code: control room, mixer snapshots, handles, QuickPunch, Capture MIDI, clips following chords, warp markers and Mackie/HUI built elsewhere; launcher, mastering page, rack and macros, browser build, EUCON, ARA, hardware DSP, cloud, loop library and vocal synthesis not planned here, with the reason |
| 2026-11-17 | minor | Plug-ins in their own process, first step: CLAP, opt-in, a crash silences the plug-in; browser recording dropped by the owner |
| 2026-11-16 | minor | Take PCs over the encrypted link only is on by default (owner); a saved choice is kept |
| 2026-11-16 | minor | Plug-ins in their own process: the design refined (not built) |
| 2026-11-16 | minor | Browser build: the session kept between visits, with a notice, Export and Clear; a design for recording |
| 2026-11-16 | minor | Remote recording: the encrypted local link and talkback |
| 2026-11-16 | minor | Timing alignment of stacked vocals built (Align timing in the Vocal Studio) |
| 2026-11-16 | minor | Clip launcher deferred by the owner; the Object/Bed switch and the Renderer say what they do not do; Ambisonics: 2nd and 3rd order encoded, 4th to 7th not offered; stored-only settings say so to agents; the video sync offset was read all along; the browser build says it cannot record; the Network window says what pairing allows; bounce to a loudness target; Scout applies a suggestion when asked; search the transcript; video export; a first-run tour; where the rest stands, and designs for the larger items |
| 2026-11-16 | major | First version: signature features of Pro Tools, Studio Pro, Logic Pro and Cubase, and what only FreeDAWm has |
