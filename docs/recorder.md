# FreeDAWm Recorder

FreeDAWm Recorder (`freedawm-recorder.exe`) is a small window that comes with FreeDAWm on
Windows. It does three things, without opening the full DAW:

1. **Quick ideas.** Press Record, play, press Stop. You get a WAV file of your audio input and,
   if you played any MIDI, a MIDI file with the same name next to it.
2. **Record into a booth.** Pair it with a FreeDAWm PC elsewhere in the building (the "booth").
   When the booth records, the recorder records too and sends the take to the booth when it
   stops, exactly as a full FreeDAWm remote recorder does.
3. **Listen to the booth's mix.** Hear what the booth hears, through this PC's output.

It opens from the Start menu (**FreeDAWm Recorder**) or as `freedawm-recorder.exe` in the
portable zip, next to `freedawm.exe`.

## Quick ideas

- **Audio**: tick it to record the audio input, pick the input device and the channel (`In 1`,
  `In 2`… for one channel, `In 1-2`… for a stereo pair). The bar under it is the input level.
- **MIDI**: every MIDI input is recorded; untick a port to leave it out.
- **Record / Stop**: the big button. The clock beside it is the take's length. Audio and MIDI
  record together.
- On Stop the window says what it saved, and the take appears in **Recent takes** with Play,
  Show (in its folder), Rename and Delete. Delete asks first and deletes for good (there is no
  recycle bin step).

### Where files go

In `Documents\FreeDAWm Ideas` (the window shows the folder; **Change…** picks another). Each take
is named by its date and time, for example:

```
Idea 2026-11-22 14-03-10.wav
Idea 2026-11-22 14-03-10.mid
```

The WAV and the MIDI file share the name so they pair up. Two takes in the same second get
`(2)`, `(3)`… after the name: a take never overwrites another file.

- The **WAV** is 24-bit at the audio device's rate (usually 48 kHz). It is written to disk while
  you record, and its header is brought up to date every second, so if the PC or the program
  crashes the take is still there and plays (up to about a second before the crash).
- The **MIDI file** is at 120 bpm with every note at the real time you played it, counted from
  the take's start (nothing is moved to a grid), so it lines up with the WAV in any program. It
  is only written when MIDI came in.
- The date and time in the names are **UTC** for now (see Limits).

## Record into a booth

On the booth (the FreeDAWm PC that keeps the session): **Setup › Network › Allow control from
this network**. Then on the recorder:

1. **Find booths** (or type the booth's address, as the booth's Network window shows it, such as
   `192.168.1.20:48100`).
2. Pick the booth and press **Pair…**. The booth shows a 4-digit code; type it on the recorder
   and press **Pair**. The code is never sent over the network: the two PCs prove it to each
   other. Over the booth's encrypted local link (on by default) the pairing also binds the two
   PCs' keys, so another computer cannot stand in for either later.
3. **Follow** is then on. The recorder shows the booth's session name and transport, records
   when the booth records, and stops when it stops.

On the booth the recorder is its own device, named `<this PC's name> Recorder` (separate from a
full FreeDAWm on the same PC). Its inputs show up on the booth's track input menus as
`Remote: <PC> Recorder › In 1` (audio) and `Remote: <PC> Recorder › <MIDI port>` (MIDI): put a
booth track on them, and the takes land on it, placed in time against the booth's clock just as
a full FreeDAWm remote recorder's would.

- Each take is also kept here, in the ideas folder, named after the booth's session
  (`Sunday Service 2026-11-22 14-03-10.wav`).
- The window says whether a take is being sent, waiting, or sent; **Outbox** counts the takes
  not yet on the booth. They are kept on this PC until the booth says it has them, also when the
  booth is away, or the recorder is closed and opened again later.
- If the booth goes away during a take, the recorder keeps recording, and sends the take when
  the booth is back.
- **Forget** unpairs (on this side; the booth can also remove it in its own Network window).

## Listen to the booth's mix

Tick **Listen**, pick the output and set the volume. The booth streams its mix while it plays
or records; the line under it says how much is buffered (milliseconds) and how many packets were
lost or late. It works whether or not you are recording. A take recorded while listening lands
where you played it against what you heard.

## Privacy and the network

- The recorder never listens on the network: it starts no server and accepts no connection. It
  only ever dials the booth you paired with.
- Nothing is sent anywhere until you ask: **Find booths** sends a local network (mDNS) query,
  **Pair…** asks the booth for a code, and once paired the recorder keeps its link to the booth
  while Follow is on.
- Over the booth's encrypted local link the recorder talks to it from a UDP socket of its own
  (which answers no one), through a bridge that only this PC can reach (127.0.0.1), as FreeDAWm
  itself does.
- Its settings are in their own files in FreeDAWm's settings folder (`%APPDATA%\FreeDAWm`):
  `recorder.json` (folder, devices, ports, volume) and `recorder-network.json` (the booth and its
  pairing, in the same format as FreeDAWm's `network.json`, which the recorder never touches),
  and the outbox in `Recorder Outbox`.

## Limits

- **Windows only** as a packaged program. (It builds and its tests run on Linux and macOS too,
  but it is not in those packages.)
- **No joining by internet invite.** The DAW's internet link asks the router to open a port and
  accepts incoming connections, which the recorder does not do. Use the local network.
- **Names are in UTC.** The standard library cannot read the PC's time zone; local time in the
  names needs a small time-zone library (awaiting the owner's approval).
- **No record offset measurement.** Takes sent to the booth are placed using the latencies the
  audio driver reports, as FreeDAWm does before a loopback measurement; the recorder has no
  measurement of its own.
- One input (a channel or a stereo pair) at a time; no input monitoring (the input is never
  heard through this PC's output).
- MIDI files are always at 120 bpm (the booth's own tempo map applies to takes sent there).
