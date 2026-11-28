# Importing a Pro Tools session

**File › Import › Pro Tools Session...** (also found by typing "pro tools" in Search; the
command is `file.import_ptx`) brings a Pro Tools session file (`.ptx`, Pro Tools 10 and later)
into the open session as new tracks. It works like File › Import › AAF...: everything it adds is
one undo step, and a report of what came across goes in the **Notes** tab, with a one-line
summary in the status line.

It reads the session file and the audio it uses. It never changes or writes the `.ptx`, and there
is no `.ptx` export.

## Before you import

- Keep the session folder as Pro Tools left it: the `.ptx` with its **Audio Files** folder beside
  it. Audio is looked for in `Audio Files` and next to the `.ptx`.
- To get the session as it was (tempo map, meters, key signatures, timecode start and frame
  rate), import into an **empty** session (File › New). A session that already has clips keeps
  its own tempo map and timecode; the Pro Tools session is then placed by its timecode start, and
  its bar-based material keeps its time in seconds, not its bars.
- Audio is referred to where it is, as File › Import › Audio does, unless
  FreeDAWm is set to copy imported audio. Audio that is not found comes in **offline** under its name: its
  clips are there, silent; relink it with Clip › Relink Audio....

## What comes across exactly

- **Tracks** in Pro Tools' order: audio (mono and stereo), MIDI, instrument, aux, routing folders,
  the master fader (into an empty session; a session with clips keeps its own master), with
  names, comments, hidden and inactive states, mute, solo and folder nesting.
- **Mixer:** fader level, pan (both pans of a stereo track), outputs and inputs (the master's
  output is the main output; other names become buses of that name), sends with level, pan,
  mute and pre/post fader, groups (edit, mix or both, on or off).
- **Automation:** volume, pan, mute and send level/pan/mute lanes.
- **Audio clips:** position, where in the file they start, length, sync point, fade in and fade
  out (equal power, or linear for Pro Tools' equal-gain fades), clip gain and its envelope, clip
  mute, alternate playlists (as extra playlists of the track).
- **Split-mono files** (`name.L` and `name.R`) on a stereo track become one stereo file in the
  session; a stereo file plays as it is.
- **MIDI clips:** notes with velocity, controllers, pitch bend, channel and poly pressure,
  program and bank changes, SysEx.
- **The timeline:** tempo changes, meters, key signatures, markers and selections (memory
  locations) with their comments, the timecode start and frame rate.

## What comes across approximately (the report says each one)

- **Track colour:** Pro Tools' colour values are not in the file and not published, so a Pro
  Tools colour number picks a colour of FreeDAWm's palette (always the same one for the same
  number). It is not a nearest-colour match.
- **Stock plug-ins with a clear FreeDAWm match** come in as FreeDAWm's own, with their settings:
  - EQ3 7-Band → CleanQ (HPF, low shelf, three bells, high shelf, LPF as bands 1–7; input and
    output gain summed into CleanQ's output). The low and high bands are taken as shelves.
  - EQ3 1-Band → one CleanQ band of the same type.
  - Dyn3 Compressor/Limiter → Compressor/Limiter (threshold, ratio, attack, release, knee,
    make-up gain, side-chain filter). The settings are copied; the two do not compress
    identically.
  - Trim → Trim (gain within 0.06 dB of Pro Tools', phase invert).
- **Loops:** a looped clip becomes looped copies (as Clip › Loop makes). When Pro Tools' loop
  length is not the clip's length, the copies are spaced by the loop length.
- **Clip groups** come in as their clips, grouped; the group's name is not kept.
- **Crossfades:** where both clips have audio past the cut, they overlap for the crossfade as in
  Pro Tools; otherwise the crossfade becomes a fade out and a fade in at the cut.
- **Basic folders** come in as folder tracks; **VCAs** without the tracks they control.
- **Timing:** notes and tempo changes between FreeDAWm's 960-a-beat ticks move to the nearest
  tick; a meter change that is not on a bar line starts at the bar it falls in; a session at
  another sample rate has its positions converted; drop-frame rates FreeDAWm lacks (30, 60, 119.88
  and 120 drop) are counted at the non-drop rate.
- Pan positions are copied; FreeDAWm's pan law applies (−3 dB centre by default).

## What is left out (listed in the report)

- Every other plug-in (reverbs, delays, Channel Strip, amp simulators, dither, third-party
  plug-ins), each named with its track and slot; instrument plug-ins, so an instrument track
  comes in without its instrument; all plug-in automation.
- Elastic Audio, track delay, MPE settings and per-note MPE expression, the LFE level, alternate
  MIDI playlists, which controls a group links.
- MXF and SD2 audio files (they come in offline).
- `.ptf` sessions (Pro Tools 7–9) and anything older are not read.

## Reading the report

The note in the Notes tab (tagged `ptx`) starts with the summary, for example:

> Pro Tools Song.ptx: 30 tracks, 30 clips, 4 markers; 2 files offline; 12 things approximated; 9 things not carried (see Notes)

then a line about the session (Pro Tools version, sample rate, bit depth, start timecode), then
one line per item, each starting with **Exact:**, **Approximated:**, **Offline:** (an audio file
not found or not readable, and why) or **Not carried:**.

## Safety

The file is read by `crates/ptx`, the session reader of ptx-session-kit (MIT), vendored and
hardened: a damaged or deliberately broken file gives an error or a partial import, never a crash
or a hang (see `crates/ptx/README.md`).

## Limits of the testing so far

The import has been tested on the two clean-room sessions that come with ptx-session-kit (made by
its own writer, not saved by Pro Tools) and on damaged copies of them. It has not yet been tried
on a session from a real Pro Tools user. Please report sessions that do not come in well.
