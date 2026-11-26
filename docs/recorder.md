# FreeDAWm Recorder

FreeDAWm Recorder (`freedawm-recorder.exe`) is a small window that comes with FreeDAWm on
Windows. It is made for getting an idea down fast, by anyone, without knowing what a DAW is:

1. **One big button.** It opens ready: press **Record** (or Space), play, press **Stop**. You get
   a recording of your microphone and, if you played a keyboard, the notes you played, side by
   side.
2. **Record with a studio.** Join a FreeDAWm PC elsewhere (the "studio"; FreeDAWm itself calls
   it the booth). When the studio records, the Recorder records too and sends the take to the
   studio when it stops, exactly as a full FreeDAWm remote recorder does.
3. **Hear the studio's mix** through this PC's speakers or headphones.

It opens from the Start menu (**FreeDAWm Recorder**) or as `freedawm-recorder.exe` in the
portable zip, next to `freedawm.exe`.

## The screens

The window is 480 × 640 when it first opens; it can be made bigger (the column stays in the
middle) or smaller (down to 380 × 520: the screen then scrolls). It opens where it was last
closed.

### 1. Ready to record

- **Record**: the big red button. Click it, or press **Space** (Space works whenever you are not
  typing in a box). It does not record on its own when the window opens, unless you turn on
  **Start recording as soon as the Recorder opens** in the settings.
- **Recording from**: what it records, in plain words: `Microphone` (the computer's default
  input, or anything named like a microphone; otherwise the device's own short name), `+ Piano
  keyboard` when a MIDI keyboard is plugged in (its name when it does not sound like a keyboard,
  `2 keyboards` for two). Every keyboard plugged in is recorded. The bar under it is the
  microphone's level, live.
- **Change** (and the sliders button at the top right) opens the settings.
- **Your ideas**: what you recorded, newest first: the name, when (`Today 2:03 pm`,
  `Yesterday`, a weekday, a date) and what is in it (`voice`, `piano notes`, `voice + piano
  notes`). The round button plays it; the rest of the row opens it (screen 3).
- **Record with a studio** opens screen 4. Once you joined a studio it shows its name, with a
  green dot when it is there and an amber one when it is away.

When the window opens, it paints first and only then opens the microphone and the keyboards (on
its second frame), so it is never slow to appear. Until they are ready the card says "Looking for
your microphone…". Pressing Record then says "Starting as soon as your microphone is ready…" and
starts the moment it is.

With nothing to record from (no microphone and no keyboard) the button is grey, the card says "No
microphone found" with "Plug in a microphone, a headset or a keyboard, then press Try again", and
**Try again** looks again. With a keyboard but no microphone it records the keyboard and says so.

### 2. Recording

The window turns red: a big clock, bars that move with the sound coming in (from the microphone
itself), a big **Stop** (or Space), what it records from, and how many piano notes you played so
far. "Saved as you go": the recording is written to disk while you play (see Files).

If the microphone is unplugged during a take, the take stops and is kept up to that moment, and
screen 3 says so. The same if the disk fills up or cannot keep up, or the speakers or headphones
stop.

### 3. Saved

- **Saved · 0:42** and a green tick (amber, with a sentence, when something cut the take short).
- **Name it (optional)**: the box shows a friendly name such as `Idea · Monday 2:03 pm`. Type a
  name and the files are renamed a second after you stop typing (or at once with Enter, or when
  you leave the box or the screen). Characters Windows cannot have in a file name (`\ / : * ? "
  < > |`) become `-` in the file name. Typing the first name back gives the files their first
  name back. A name another idea already has is refused, and the box says so.
- The big **Play** button plays it; beside it is the recording's real shape (read from the file).
  The line under it says what is in it ("Your voice, plus the piano notes you played (23
  notes)").
- **Record another** (or Space) starts the next idea.
- **Open in FreeDAWm** starts FreeDAWm (`freedawm.exe`, next to the Recorder) with the idea in a
  new, untitled session: the recording on an audio track and the notes on an instrument track,
  both at the very start. When FreeDAWm is not next to the Recorder the button says "FreeDAWm not
  found".
- **Show the file** opens its folder with the file selected.
- **Kept in Documents › FreeDAWm Ideas** says where it is.
- **All my ideas** goes back to screen 1. **Throw away** asks first ("Throw this idea away? It
  can't be undone.") and then deletes both files for good (there is no recycle bin step).

An idea opened from the list shows the same screen, with when it was made at the top.

### Layers: play an idea round and round and record another part over it

On screen 3, beside **Record another**, is **+ Add a layer**:

1. Press it. The screen turns red and counts **3, 2, 1** (0.8 s each; **Cancel**, Space or
   Escape stops it). The count can be turned off in the settings: the layer then starts at once.
2. The idea plays from its start, round and round, and whatever you play now is recorded: the
   screen says "Adding a layer to Chorus idea", shows **Layer 2** with a bar for where the idea
   is in its loop, and "Round 1", "Round 2"… The first time, an amber line says "Use headphones
   so the first part isn't recorded again" (nothing stops you: the new layer holds only what
   comes into the microphone and keyboards, never what is played back, but a microphone hears
   the speakers).
3. Press **Stop** (or Space). Screen 3 says "Layer 2 saved" (or "Layers 2 and 3 saved").

**Each time round becomes its own layer** (like a looper): playing through the loop three times
gives layers 2, 3 and 4. Every layer file is exactly as long as the idea and starts at the idea's
start, so all of them loop cleanly and line up when dropped at the same place in any program. A
last time round stopped half way is filled out with silence; a time round in which nothing was
played (no sound above −45 dBFS and no note) is not kept ("1 time round with nothing played was
not kept."), and the numbers stay without a gap. Pressing Stop before anything was played adds
nothing and says so. (A single long file per layer was considered and left out: every file the
same length is what loops and lines up, and it needs no setting.)

The idea's screen then shows **3 layers, playing together** and one row per layer: **Mute**
(amber **Muted** when muted), its name and what is in it (`First take · voice`, `Layer 2 · notes`),
its level (a slider: the middle is as recorded) and **×** to throw that layer away, which asks in
its row first (**Keep** / **Throw away**). The **Loop** button beside the shape plays the idea
round and round (on once it has layers; off for a single take, which plays once as before).
**Play** plays every unmuted layer together at its level; a new layer always records against what
is unmuted. Muting, levels and Loop are remembered in a small file beside the idea (below).

**Throw away** on the idea asks "Throw this idea away, all 3 layers?" and deletes every layer and
the small file. **Renaming** the idea renames all its layers. In the list an idea with layers is
one row: `Chorus idea · Today · 3 layers · 0:42`. **Open in FreeDAWm** opens every layer, each on a
track of its own named after its file (`Chorus idea`, `Chorus idea (layer 2)`…), in layer order,
all at the start. (Mute and level are not passed to FreeDAWm.)

**Lining up.** A layer must line up with what you heard. The Recorder moves each layer earlier by
the time sound takes to go out of the speakers and back in (as FreeDAWm places its takes):

- Without anything set, the latencies the audio drivers report are used. These are usually right
  to within a few milliseconds; some drivers leave out converter or Bluetooth delays, so a layer
  can sound slightly late (Bluetooth headphones: very late).
- **Line up** (settings, LAYERS) plays a click and listens for it, with a cable from output to
  input or the microphone near the speaker, as FreeDAWm's own Measure does. It finds the round
  trip to the sample with a cable (the tests line up a re-recorded click within 2 samples), and to
  about a millisecond with a microphone (plus about 3 ms per metre between speaker and microphone).
  It is kept for those devices, buffer and rate.
- **Earlier / Later** (5 ms steps, ±500 ms) moves layers by hand, on top of either.

MIDI played over the loop is placed by the output's latency as a take's is (it is heard late by
that much). A note held across the loop's end is cut there.

### The click (metronome)

Off unless turned on: the Recorder stays "press Record and play". In the settings, **CLICK**:

- **Play a click while recording**. When on: the tempo (type it, **−** / **+**, or **Tap** along
  a few times: the gap between the last taps, up to five, sets it; a pause of over 2 seconds starts
  again), **Beats in a bar** (3, 4 or 6; the bar is x/4), the click's volume, and **Make a new
  idea whole bars, so it loops in time** (on).
- The click is FreeDAWm's own metronome sound (accented on the bar's first beat). It plays while a
  take or a layer records, only to the speakers or headphones: it is never in any file (use
  headphones so the microphone doesn't hear it).
- Before a take, or a layer, one bar of it counts in: the screen shows the beat ("Get ready: a bar
  of click at 96 BPM, then it records") and the take starts after it. This replaces the 3-2-1
  before a layer.
- An idea recorded to the click knows its tempo: its MIDI file is written at that tempo and time
  signature (rather than 120 bpm in 4/4), its `.fdidea` keeps the tempo, its layers click at it,
  and with **whole bars** on its WAV is made the nearest whole number of bars when it stops (filled
  out with silence, or cut with a 10 ms fade), and the screen says "Looped as 4 bars".
- Without the click everything is as before.

### 4. Record with a studio

On the studio (the FreeDAWm PC that keeps the session): **Setup › Network › Allow control from
this network**. Then, on the Recorder, **Record with a studio**:

- **Studios found on this network** are listed, each with a big **Join**. Looking for them
  starts when this screen opens, not before (nothing at all is sent on the network until you open
  it), and stops when you leave it.
- **Join** asks that studio. If the studio has a booth PIN, the box asks for it ("Type Studio A's
  PIN"); otherwise the studio shows a 4-digit code and the box asks "Type the code showing on the
  studio's screen". Type it and press Join (or Enter). The code or PIN is never sent over the
  network: the two PCs prove it to each other. If the studio also asks someone there to press
  Allow, the screen says so.
- **Somewhere else? Paste the PIN or invite they sent you**: the same box takes
  - a booth PIN (for the studio found here, or the one you asked last),
  - an internet invite (`freedawm-invite:…`, made on the studio with **Invite…** in Setup ›
    Network), to join a studio somewhere else over the internet,
  - or a studio's address as its Network window shows it (`192.168.1.20:48100`).
- Problems are said in plain words: "That didn't match. Check the code or PIN and type it again.",
  "Can't reach the studio…", "That invite has run out. Ask the studio for a new one.".

Once joined, the screen shows the studio and its song, what it is doing ("Waiting for the studio
to press Record", "Recording with the studio", "Sending your take… 40%", "Sent", "The studio is
away. Waiting for it…", with how many takes are kept here until it has them), your microphone's level, and **Hear the studio's
mix** with its volume (half way is as loud as the studio sends it). **Leave this studio** (here
and in the settings) forgets it on this side; the studio can also remove it in its own Network
window.

While the studio records, the Recorder shows screen 2 ("Recording with Studio A"); when it stops,
the take is sent and the studio screen says how far it got. Takes not yet on the studio are kept
on this PC (also when the studio is away, or the Recorder is closed and opened again later) and
sent when it is back.

On the studio the Recorder is its own device, named `<this PC's name> Recorder` (separate from a
full FreeDAWm on the same PC). Its inputs show up on the studio's track input menus as
`Remote: <PC> Recorder › In 1` (audio) and `Remote: <PC> Recorder › <MIDI port>` (MIDI): put a
track on them, and the takes land on it, placed in time against the studio's clock just as a
full FreeDAWm remote recorder's would. Each take is also kept here, in the ideas folder, named
after the studio's session.

### Settings

The sliders button (or **Change**) opens them; **Done**, **Back** or Escape closes them.

- **Microphone or input**: the computer's own (default) or another device; **which input** on it
  (one, or a stereo pair); **Record sound from it** (off: keyboards only).
- **Keyboards**: every one plugged in, each with a tick (all on; one plugged in later records too).
- **Where ideas are kept**, with **Choose…**.
- **Play back through**: the speakers or headphones that play ideas and the studio's mix.
- **Start recording as soon as the Recorder opens** (off unless you turn it on).
- **Layers** (at the end): **Count 3-2-1 before a layer starts** (on), **Earlier / Later**, and
  **Line up** (see Layers).
- **Studio**: the studio joined, **Leave this studio**, and the name the studio sees.

The lists of devices are read in the background when the settings open.

## Keys

- **Space**: Record / Stop (not while typing in a box, and not on the studio and settings
  screens). On an idea's screen it is **Record another**; while a layer counts in or records it
  cancels or stops it.
- **Enter** in the name box: rename now. In the studio box: Join.
- **Escape**: back from the studio screen, the settings or an idea.
- **Tab** reaches every button, box and tick in turn (a blue ring shows which); Enter or Space
  presses it. Every button is at least 44 points tall.

## Files

In `Documents\FreeDAWm Ideas` (the settings can choose another folder). Each take is named by
its date and time **in the computer's local time**, for example:

```
Idea 2026-11-23 14-03-10.wav
Idea 2026-11-23 14-03-10.mid
```

The window shows that as `Idea · Monday 2:03 pm`. The WAV and the MIDI file share the name so
they pair up. Two takes in the same second get `(2)`, `(3)`… after the name: a take never
overwrites another file.

- The **WAV** is 24-bit at the audio device's rate (usually 48 kHz). It is written to disk while
  you record, and its header is brought up to date every second, so if the PC or the program
  crashes the take is still there and plays (up to about a second before the crash).
- The **MIDI file** is at 120 bpm with every note at the real time you played it, counted from
  the take's start (nothing is moved to a grid), so it lines up with the WAV in any program. It
  is only written when MIDI came in.
- **Layers** are `Idea 2026-11-23 14-03-10 (layer 2).wav` / `.mid`, `(layer 3)`… beside the
  idea, each exactly the idea's length from its start. Files named that way by hand are taken as
  layers too. `Idea ….fdidea` (a small JSON file) remembers each layer's mute and level and the
  Loop button; it sits beside the files, so it goes with them when the folder moves, and nothing
  else needs it. While a layer records, its sound is written to `(layer n).wav` as it goes (a
  crash keeps it, playable) and the `.fdidea` notes it: the next time the Recorder looks at the
  folder it cuts that file into its layers. The MIDI of a layer being recorded during a crash is
  lost (as a take's is). Earlier layers are never rewritten. A MIDI file's track has no name of
  its own, so FreeDAWm names the track after the file.
- **Local time.** On Windows the time zone's offset is the one in force now (Windows' local clock
  less its UTC clock), so the names of takes made now are right; the "Today/Yesterday" words for
  an older idea renamed by hand (which come from its file's time) can be an hour off across a
  daylight-saving change. On macOS and Linux the offset is the one at each time.

## Privacy and the network

- The recorder never listens on the network: it starts no server and accepts no connection. It
  only ever dials the studio you joined.
- Nothing is sent anywhere until you ask: opening the studio screen sends a local network (mDNS)
  query, and only while it is open; **Join** asks the studio for a code; once joined the
  recorder keeps its link to the studio.
- Over the studio's encrypted local link the recorder talks to it from a UDP socket of its own
  (which answers no one), through a bridge that only this PC can reach (127.0.0.1), as FreeDAWm
  itself does.
- **Internet invites** are joined from a *dial-only* endpoint, made only when you join with an
  invite (or to reconnect to a studio joined with one): it offers no protocol, so a connection
  made to it is refused during its handshake and nothing ever accepts one; it asks no router to
  open or map a port (no UPnP, NAT-PMP or PCP); and it publishes its address nowhere. It reaches
  the studio directly when it can, else through the relay the studio uses (the invite names it;
  n0's public relays unless the Recorder's network settings name another), through a 127.0.0.1
  bridge as above. Its key is kept in `recorder-network.json`.
- Its settings are in their own files in FreeDAWm's settings folder (`%APPDATA%\FreeDAWm`):
  `recorder.json` (folder, devices, ports, volume, start-up, the window's place) and
  `recorder-network.json` (the studio and its pairing, in the same format as FreeDAWm's
  `network.json`, which the recorder never touches), and the outbox in `Recorder Outbox`.

## Limits

- **Windows only** as a packaged program. (It builds and its tests run on Linux and macOS too,
  but it is not in those packages.)
- **The font** is Inter Regular, the copy FreeDAWm already ships; bold text is that face drawn
  slightly thicker, as only the regular weight is bundled.
- **Record offset.** Takes sent to a studio are placed using the latencies the audio driver
  reports, as FreeDAWm does before a loopback measurement. **Line up** is used for layers only.
- **Layers** work on ideas up to 10 minutes long: the idea's layers are read into memory to play
  (about 23 MB a minute), and a layer take is read back whole at Stop to cut it. Layering is for
  quick ideas only, not while recording with a studio.
- One input (a channel or a stereo pair) at a time; no input monitoring (the input is never
  heard through this PC's output).
- MIDI files are at 120 bpm, or at the click's tempo when it is on (the studio's own tempo map
  applies to takes sent there). **Open in FreeDAWm** takes an idea's tempo from its MIDI file
  (FreeDAWm sets the session's tempo map from it); an idea with no MIDI file opens at FreeDAWm's
  own tempo, so its audio is not yet on FreeDAWm's grid.
- No built-in instrument yet: a keyboard is recorded to a MIDI file and is not heard in the
  Recorder.
