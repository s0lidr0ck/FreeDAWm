# Four-DAW parity, section 8: what only one of them has

> **Last reviewed:** 2026-11-16 · **Last updated:** 2026-11-16 · **Change:** minor (tier 4) · **Compared with:** Avid Pro Tools 2026.4.1, Fender Studio Pro 8.2 (formerly PreSonus Studio One), Apple Logic Pro 13.0, Steinberg Cubase Pro 15

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
  only, the object/bed switch is read only by drawing code, timing alignment in Fit to Lead is
  refused, nothing outside the video crate calls its encoder.
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
  Dolby Atmos objects and renderer, AAF/OMF exchange, a control room with cue mixes, modulators,
  articulation maps, a loop library, control surfaces, ARA.
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
| Hardware DSP mixing with near-zero monitoring latency (HDX, Carbon) | **Missing** | A software low-latency monitoring path, for audio tracks |
| AAF and OMF exchange with video editors | **Missing** | DAWproject both ways instead |
| Import chosen tracks and settings from another session | **Works** | Import Session Data, with audio and sampler samples |
| Field-recorder workflow: match and expand location recordings by metadata | **Missing** | Broadcast WAV description is read and written; no iXML, no matching |
| Clip effects: EQ and dynamics per clip, live | **Works** | Per-clip EQ, compressor and gain, with copy, paste and render |
| Beat Detective across many tracks, phase kept | **Partial** | Finds hits across selected tracks and cuts all at the same points; conforming is done by Detect Tempo › Straighten; no trigger editing or edit smoothing |
| Sketch: a clip-launcher window, also on a tablet | **Missing** | DAWproject import drops launcher clips and says so |
| Dolby Atmos renderer built in: beds, objects, re-renders, ADM; MPEG-H | **Partial** | Channel surround to 9.1.6 with a 3-D panner works. Object/Bed is a mark that says it is one; no renderer, no ADM |
| Speech-to-text on dialogue with text search | **Partial** | Lyric transcription with word times, searchable (AI › Find in Transcript…); no dialogue-specific lane |
| Video: several tracks, editing, satellite link, pull up and down | **Partial** | One video track with H.264, ProRes and Motion JPEG playback, timecode and feet+frames; export to a ProRes movie with the mix; no editing |
| Destructive record, QuickPunch, TrackPunch | **Flag only** | Ordinary range punch works; the three modes are stored and unread |
| EUCON control surfaces | **Missing** | No control-surface protocol of any kind |
| Scripting SDK | **Works** | Every command, from a script, a command line or an agent (section F) |
| Cloud collaboration on one project | **Missing** | |
| AudioSuite with handles; commit, freeze, bounce | **Partial** | Process renders a new clip without handles; freeze (also up to a chosen insert), commit and bounce work |
| Track Pin, batch rename and other large-session aids | **Partial** | Not checked item by item; no pinned tracks |

## B. Studio Pro (Studio One)

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Project page: master an album in the same app, songs update from their mixes, DDP and digital release | **Missing** | Bounce normalises to peak and reports loudness |
| Show page: set lists for playing live | **Missing** | |
| Scratch pads: try another arrangement beside the song | **Missing** | Playlists per track are the nearest thing |
| Arranger track: drag sections to reorder the song | **Partial** | Sections exist for the Band; moving one does not move track audio or MIDI |
| Chord track that re-pitches audio as well as MIDI | **Partial** | Chords drive the Band and re-pitch single-line vocals (Fit to Lead, Make Harmony); existing MIDI clips and chordal audio do not follow |
| Mix Engine FX: one console model across every channel of a bus | **Missing** | |
| Splitter and extended chains: parallel and band-split routing inside the inserts, macro knobs | **Missing** | Inserts are a straight chain; linked plug-ins share values, which is not a macro |
| Sound variations (articulations for orchestral libraries) | **Missing** | Keyswitches exist only inside the Band's instrument profiles |
| Launcher: clips and scenes tied to the arrangement | **Missing** | |
| Lyrics track, lyrics in the score and on a display | **Partial** | Lyrics lane and sheet, a teleprompter for the singer, rhymes and word tools; no lyrics in the score |
| Drag and drop for everything | **Partial** | Plug-in to track or to empty space, audio to sampler, files from the system; nothing drags out |
| Global transpose; audio follows tempo by default | **Partial** | No global transpose; tempo-following is a per-clip choice |
| Listen bus with room correction | **Missing** | |
| Amp and pedal models from a guitar-amp maker | **Works** (unbranded) | Seven amps solved from schematics, cabinets, pedal circuits |
| Tablet remote app | **Partial** | A phone web page on the local network: transport, record, arm, markers; no mixing |
| Built-in assistant, stem separation and vocal tuning (8.1), MIDI 2.0 (8.2) | **Partial** | Stem separation and vocal tuning work; assistant: see Scout (section F); no MIDI 2.0 |

## C. Logic Pro

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Session Players: drummer, bass and keyboard players that follow chords | **Works** | The Band: six players chosen from 152 styles of player, following sections and chords, printable to MIDI. Sounds come from the track's instrument, and FreeDAWm ships few |
| Live Loops grid | **Missing** | |
| Smart Tempo: record without a click, then fit project or audio | **Works** | Detect Tempo (constant, follow, smooth) writes the tempo map; Straighten conforms audio; no free-tempo record mode as such |
| Flex Time and Flex Pitch on the track | **Partial** | Pitch in a separate vocal editor; time by clip stretch and by markers that Straighten writes; no hand-placed flex markers |
| Stem Splitter, now down to single drums | **Works** | Four or six stems, lead and backing vocals, and drums into kick, snare, toms, hat, ride, crash. Models are downloaded |
| Mastering Assistant | **Partial** | Scout measures the mix and returns advice, and applies it a step at a time when asked; no single-press mastering chain |
| Drum Machine Designer, Alchemy, Sculpture and the rest of the instrument set | **Partial** | A capable drum machine and a one-sample sampler; nothing like the synths ([section 7](07-midi-writing.md#l-instruments-in-the-box)) |
| Smart Controls; tablet remote with touch instruments | **Partial** | A few key knobs per insert on the mixer; transport-only phone remote |
| Track Stacks and patches | **Works** | Routing folders sum their tracks; track and chain presets carry their samples |
| Scripter and the Environment | **Missing** | Scripting is from outside the app only |
| Spatial audio with binaural monitoring and head tracking | **Partial** | Surround panning; no binaural path |
| A very large sound and loop library; loops follow tempo and key; one browser for everything | **Missing** | The browser lists plug-ins and presets; shipped content is about 20 MB |
| Round trip with the tablet edition and with MainStage | **Missing** | |
| Chord ID from audio | **Works** | Detect Chords writes the chord lane |
| Flashback capture | **Flag only** | See [section 6](06-midi-recording.md#g-punch-count-in-and-capture) |
| Step Sequencer with note and automation rows; articulation sets | **Partial** | Drum step sequencer; no automation rows, no articulations |
| Assistant that operates the app from a prompt (13, by subscription) | **Partial** | No assistant inside the app; an outside agent can operate every command (section F) |

## D. Cubase

| Signature feature | FreeDAWm | What is there |
|---|---|---|
| Expression Maps and Note Expression | **Missing** | |
| VariAudio: pitch and formant editing, harmonies from the chord track | **Works** | Note-based tuning with formant-preserving resynthesis; Make Harmony follows the chords; a view of every vocal track with clashes marked |
| Chord Pads and Chord Assistant | **Partial** | A chord picker offering the key's triads and sevenths; no pads, no suggestions |
| Control Room: cue mixes, talkback, monitor sets | **Missing** | |
| Mixer undo history and mixer snapshots | **Partial** | Mixer moves are ordinary undo steps; no snapshots |
| Logical Editor, Project Logical Editor, macros | **Missing** | |
| MIDI Remote: scripted controller support with a mapping assistant | **Missing** | |
| Sampler Track with slicing | **Works** | Drop audio on an instrument track or send a selection to the sampler; slices by hits, beats or equal parts |
| Audio Alignment of stacked takes | **Missing** | The option exists in Fit to Lead and is refused with a message |
| Hitpoints, AudioWarp, Time Warp | **Partial** | Detection, Straighten and Conform; no free warp tool |
| Direct Offline Processing: an editable offline chain per clip | **Partial** | One-shot Process; Clean Vocal keeps the original with an amount you can change later |
| Modulators for any parameter | **Missing** | |
| Track Versions, arranger chains, Render in Place | **Partial** | Playlists; commit and bounce; no arranger chains |
| Remote recording over the internet (VST Connect) | **Partial** | Works, by a different design (section F); no talkback, no video |
| ARA 2 and spectral editing | **Missing** | A view flag exists and is unread |
| Score editor with engraving; drum and pattern editors, now melodic | **Partial** | A read-only score with print and MusicXML export; drum step sequencer only |
| Video export; loudness track and normalising on export; metering suite | **Partial** | Bounce to a loudness target with a true-peak ceiling; export to a ProRes movie; no loudness track; meters are strong (BS.1770 with true peak, LevelUp, Scout) |
| Vocal synthesis (Omnivocal, beta) | **Missing** | Song generation produces finished audio, not an editable sung part |

## E. Where three or four now agree

These started as one product's signature and have spread. They are the gaps most likely to be
noticed, and the ones sections 1–7 did not already list:

| Feature | PT | SP | Logic | Cubase | FreeDAWm |
|---|:-:|:-:|:-:|:-:|---|
| Clip launcher | ● | ● | ● | ○ | **Missing** |
| Stem separation | ● | ● | ● | ● | **Works** |
| Sound and loop service or library in the browser (Splice or the maker's own) | ● | ● | ● | ● | **Missing** |
| Dolby Atmos mixing with a renderer | ● | ● | ● | ● | **Partial** (see A) |
| Arranger sections that move the song | ○ | ● | ● | ● | **Partial** (Band only) |
| Chord track that existing clips follow | ◐ | ● | ● | ● | **Partial** |
| ARA 2 | ● | ● | ● | ● | **Missing** |
| An assistant inside the app | ○ | ● | ● | ○ | **Partial** (Scout advises on the mix only) |
| Tablet or phone remote | ● | ● | ● | ● | **Partial** |

## F. What FreeDAWm has that none of the four does

Each was checked in the code. "Works" means wired end to end; none was listened to or timed.

| Feature | Status | What it is, and its limits |
|---|---|---|
| Every command open to scripts and AI agents | **Works** | About 330 commands with documented parameters. A command-line tool runs them on a session without the window; a running app accepts them on a local port; an MCP server lets an AI agent list commands, run them, look at the session, take screenshots and click. Any menu item can be invoked by its path |
| Remote recording with the take landing in place | **Works** | A second computer follows the transport, records locally and sends the take (lossless audio, or MIDI). The performer hears the live mix; clocks are compared during the take so the audio lands where it was played. Works on a local network or across the internet (encrypted, by one-time invite, no account). Takes survive a dropped connection or a crash. No talkback or video. On a local network the link is not encrypted, and a paired computer can run any command; the Network window says both |
| Scout: measured mix advice from an AI model | **Works** | Captures loudness, true peak, spectrum, stereo and transient measures of a mix or a channel, sends them with the channel's settings to Claude using the user's own key, and draws the returned EQ suggestions on the EQ as guides. It changes nothing by itself; each suggestion can be applied on request, one undo step each. Reference profiles can be built from other recordings, per stem |
| Local models beyond stems | **Works** | Lead from backing vocals; drums into six parts; audio to MIDI by four models (general, piano with pedal, drums, voice) with a live preview; MIDI from a text description; a whole song from a prompt. All run on the user's machine after a download; each model's licence is shown, and non-commercial ones ask for confirmation |
| Mend: vocal de-noise and de-reverb on a live insert | **Works** | The same model as Clean Vocal, running in the chain with about 2.9 s latency or a lighter live mode. Desktop only |
| MIDI Cleanup against key and chords | **Works** | Corrects a transcribed part without changing starts, velocities or pitches of kept notes; switchable until committed |
| Vocal Studio: every vocal on one pitch view | **Works** | Marks clashes and unisons across tracks, moves clashing notes to fit the lead, writes a harmony. Timing alignment is not done |
| ChamberMade; Imprint and Vault | **Works** | A reverb from a room you draw (size, materials, objects, speaker and microphone); capture of an impulse response from any plug-in, outboard unit or room, then convolution with it |
| Rez, Manners, Lull's guard, Shale | **Works** | A resonance suppressor; an EQ that reduces masking between bands by itself; a gate that listens to the source it should ignore; a drum gate that learns bleed, links across the kit's microphones and can send its hits as MIDI |
| LevelUp | **Works** | Loudness-matched before-and-after listening by a linked pair of inserts, with reference tracks and a preview of what streaming services do to the level |
| Lyric tools | **Works** | Rhymes, synonyms and stress checks offline; alternative lines; a teleprompter; timed lyrics from a recording |
| Browser build | **Partial** | The same app in a web page: playback, editing, built-in effects, save and export as downloads. No recording, hardware MIDI, third-party plug-ins, models or networking, and nothing is kept between visits |
| Its own video decoders | **Works** | H.264, ProRes and Motion JPEG without system codecs; its own ProRes encoder writes the movie export |
| Open source, five platforms | **Works** | MIT or Apache-2.0 (the Windows build with ASIO is GPLv3). Linux, macOS, Windows, FreeBSD and the web. English only; little work on screen readers |
| Take recovery and safety by tooling | **Works** | A running take is recovered after a crash; ten backups per session; the build refuses code that can panic outside tests. A hosted plug-in still runs inside the app and can bring it down |

## Things that promise more than they do

| # | What | Where | Status |
|---|---|---|---|
| 1 | Object/Bed switch on the mixer strip and the Renderer window: the switch is stored and drawn; no audio path reads it, and the Renderer only draws speakers and dots | `engine/src/cmd/track_more.rs:21-40`; `ui-egui/src/mix_window.rs:408-414`; `ui-egui/src/extra_windows.rs:86-145` | **Labelled** (943143a): the switch's hover, its command's description and the Renderer window say that objects are not rendered yet and play through their panners into the bed. Rendering objects is the mixing section's work ([section 3](03-mixing.md)) |
| 2 | Track widths of 2nd- to 7th-order Ambisonics can be chosen; the panner fills the first four channels and leaves the rest silent | `dsp/src/pan.rs:268-279` | **Done** (a2e9845): the panner encodes every order the track carries (ACN, SN3D, to 7th order in the DSP), so 2nd and 3rd order are filled; the mixer carries at most 16 channels, so 4th to 7th order are no longer offered and are refused by the commands that make tracks, busses and outputs. The decoder to speakers is still first order |
| 3 | QuickPunch, TrackPunch, Destructive Record and Destructive Punch have keys or commands and store a mode nothing reads; Prepare DPE Tracks consolidates for a punch that never happens | `engine/src/cmd/options.rs:22, 58-59, 125-130`; `engine/src/cmd/setup_more.rs:22, 130` | **Another section's**: punch modes are the recording work in [section 2](02-recording-audio.md); QuickPunch landed there (4b98b7f) |
| 4 | Stored and unread: transcription lane and settings, scene and take in clip names, ARA note overlay, peripherals, video sync offset, Ableton Link in the transport, calibration mode, auto-spot | `engine/src/cmd/view_more.rs:145, 154`; `engine/src/cmd/setup_more.rs:31-53` | **Done; claim partly wrong** (8fac425): these were already off the menu bar (the 2026-10 menus clean-up); agents and scripts still reach them, and their descriptions now end "stored only: nothing in FreeDAWm reads this setting yet". The video sync offset is not one of them: the Video track and window read it (it has no window of its own) |
| 5 | The web build has no way to record and no place that says so; MIDI windows are shown there with nothing behind them | `apps/soundcraft-web`; `ui-egui/src/lib.rs:996-999` | **Done for recording** (b5ef0ca): in the browser build Record says it cannot record there, on the button and in the status line. The MIDI windows are the MIDI recording section's ([section 6](06-midi-recording.md), row 23), which left them out of the web build's menus (352090a) |
| 6 | On a local network, a paired computer can run any command over an unencrypted link. The pairing code is four digits with attempt limits. Fine for a studio network; worth saying in the window, and worth offering the encrypted path locally too | `net/src/hub.rs:633, 794, 1026`; `net/src/pairing.rs:1-30` | **Done** (7ed1bfd): the Network window and the pairing prompt say that a paired device can do everything the user can, and that the local link is not encrypted while the internet one is. The encrypted local link is tier 4 |
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
| Arranger sections that move everything, and clips that follow the chord track | 14–22 | Sections and chords already exist for the Band; this makes them act on the user's own tracks. Also in [section 7](07-midi-writing.md) | **Not started here**: section 7's work (its rows are still Missing); not begun so as not to cross it |
| Bounce to a loudness target; video export | 10–16 | Both have their parts already (loudness meter; muxer and encoder) | **Loudness target done** (ca44ec5): Bounce Mix takes a target in LUFS (integrated, BS.1770) with a true-peak ceiling (default -1 dBTP), measured in a first pass; gain only, so a mix that would pass the ceiling stops short and says by how much. Video export **done** (3b781d6): File › Export › Movie (ProRes)… writes the Video tracks' picture and the main mix as a QuickTime movie, ProRes 422 (proxy, LT, standard or HQ) at the first movie's size and frame rate, with 24-bit PCM stereo |
| Scout builds a starting chain from its advice, with the user's consent per step | 8–12 | Turns advice into Logic's Mastering Assistant, on any channel | **Done** (06244b1): each suggestion of an answer has an Apply button; `scout.plan` says what each would do, `scout.apply` does one, as one undo step. An EQ guide becomes a band on the track's Clean Q (one is added if missing); a module suggestion sets its parameter to the middle of the suggested range when that range is in the parameter's own units, and is refused otherwise. Uses the answer already received: nothing more is sent |
| Timing alignment of stacked vocals | 10–16 | The pitch half is done; the command already has the option | **Not started**: it needs the note-level timing the vocal edits do not have (see the design below) |
| Search the transcript; transcript lane | 6–10 | Whisper output is there; Pro Tools' dialogue workflow | **Search done** (ade867d): `lyrics.search` lists every place a phrase is sung or spoken across the tracks' timed lyrics (case, punctuation and line breaks ignored); AI › Find in Transcript… selects the next one and moves the playhead there. The lane is the lyric strip each track already draws; a separate taller dialogue lane is not built |
| Macro knobs and a splitter in the insert chain | 14–22 | Also in [section 5](05-built-in-effects.md)'s list; count once | **Another section's** ([section 5](05-built-in-effects.md)) |
| Modulators for any parameter | 10–16 | Also in section 5's list | **Another section's** ([section 5](05-built-in-effects.md)) |
| Control room: cue mixes from sends, talkback, monitor sets, listen bus | 14–22 | Pairs with remote recording, which has no talkback | **Not started**: section 3 has it not started; left for later, after this section's own items |
| Mixer snapshots | 6–10 | | **Not started**: as the control room |
| Clip launcher: cells, scenes, record the performance to the timeline | 40–60 | Three of the four have one. The largest item; needs a design first | **Deferred by the owner** (2026-11-16): not to be built for now |
| A mastering page: several songs in order, loudness per song, export a release | 24–36 | Only Studio Pro has it; a strong reason people choose it | **Not started**: design below |
| Atmos: objects and beds routed for real, ADM export, a binaural monitor | 30–50 | A renderer of FreeDAWm's own; Dolby's needs a licence. Also in [section 3](03-mixing.md) | **Another section's** ([section 3](03-mixing.md)) |
| ARA 2 hosting | 30–45 | Also in [section 4](04-editing-audio.md) | **Another section's** ([section 4](04-editing-audio.md)) |

**3. Not worth chasing, or not possible.** Hardware DSP mixing and EUCON (tied to one maker's
hardware); cloud collaboration and tablet companions (a service and second app to run); a large
sound library (content and licences, the owner's call); AAF and OMF (large formats with little
public documentation; DAWproject covers the modern case); a live-show page and field-recorder
workflow (specialist; revisit if users ask); vocal synthesis.

**4. Build on what only FreeDAWm has (≈ 40–65 h).** Recording in the browser build, with
sessions kept between visits (12–18; not started, design below); talkback and encrypted local links for remote recording
(10–16; not started, design below); isolate hosted plug-ins in their own process so a crash cannot take the session (16–28,
and the scan helper is a start; not started, design below); a first-run tour (4–6): **done** (a689967), seven cards opened once on the first start (no settings saved yet) and again from Window › More › Tour of FreeDAWm.

**Designs for the larger items, not built yet.** Each was judged too big to finish well in the
same pass as the items above; these are the starting points.

- *Mastering page (24–36 h).* A release file of its own (`.fdrel`, JSON) beside sessions: an
  ordered list of songs, each a session (bounced on demand, so a song follows its mix) or an audio
  file; per song a gain or loudness target (the bounce's two-pass measurement, already there), a
  gap and fades; one shared master chain plus a chain per song, run offline through
  `RangeRender`. A `release.*` command family (add, move, remove, set, render) and a page that
  shows the songs in a row with their integrated loudness, true peak and the album's. Export:
  one file per song plus one continuous file with a cue sheet; DDP later.
- *Recording and sessions kept in the browser build (12–18 h).* Input through `getUserMedia`
  into an `AudioWorklet` that posts 128-frame blocks over a `MessagePort` (a `SharedArrayBuffer`
  ring needs cross-origin isolation from the host, which the page cannot assume), into the same
  `Recorder` take buffer the desktop uses; latency from `AudioContext.outputLatency` plus
  `baseLatency`. Sessions and takes kept in the origin-private file system (OPFS) as the autosave
  folder, so a visit picks up where the last left off; the Record refusal added here goes away.
- *Encrypted local link (part of 10–16 h).* Reuse the internet path's QUIC (TLS 1.3) on the local
  network instead of the plain TCP link: each side keeps a self-signed key; pairing runs a
  password-authenticated key exchange (SPAKE2 or similar) over the 4-digit code, which binds both
  keys, so a machine in the middle of the first contact learns nothing and cannot pass; later
  connections pin the keys. QUIC needs a UDP socket beside today's TCP listener: a new listening
  socket, so the owner decides before it is built. Talkback then rides the existing live-mix
  stream in the other direction.
- *Plug-ins in their own process (16–28 h).* One helper process per hosted plug-in (the scan
  helper's launcher is the start), audio through shared memory, one block of added latency so
  the audio callback never waits: it hands block *n* over and takes block *n − 1*'s result; a
  block not back in time is silence and a counted overrun. Parameters, state and the editor
  window over a pipe. A helper that dies bypasses its slot, says so, and is restarted from the
  last stored state on request.
- *Timing alignment of stacked vocals (10–16 h).* Pair each note of the backing part with the
  lead note it overlaps; a warp map from the backing note starts and ends to the lead's, limited
  to a set distance; rendered as Straighten renders warp markers, with the tuning analysis
  re-run on the result (note edits are tied to the audio they were made on, which is why this is
  not a small change).

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
| 2026-11-16 | minor | Clip launcher deferred by the owner; the Object/Bed switch and the Renderer say what they do not do; Ambisonics: 2nd and 3rd order encoded, 4th to 7th not offered; stored-only settings say so to agents; the video sync offset was read all along; the browser build says it cannot record; the Network window says what pairing allows; bounce to a loudness target; Scout applies a suggestion when asked; search the transcript; video export; a first-run tour; where the rest stands, and designs for the larger items |
| 2026-11-16 | major | First version: signature features of Pro Tools, Studio Pro, Logic Pro and Cubase, and what only FreeDAWm has |
