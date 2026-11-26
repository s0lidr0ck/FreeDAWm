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
- **Studio**: the studio joined, **Leave this studio**, and the name the studio sees.

The lists of devices are read in the background when the settings open.

## Keys

- **Space**: Record / Stop (not while typing in a box, and not on the studio and settings
  screens).
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
- **No record offset measurement.** Takes sent to a studio are placed using the latencies the
  audio driver reports, as FreeDAWm does before a loopback measurement; the recorder has no
  measurement of its own.
- One input (a channel or a stereo pair) at a time; no input monitoring (the input is never
  heard through this PC's output).
- MIDI files are always at 120 bpm (the studio's own tempo map applies to takes sent there).
