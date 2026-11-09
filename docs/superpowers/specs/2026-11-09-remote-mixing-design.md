# Remote mixing

Status: step 1 built (LAN). Owner decisions: LAN first, the internet through the same code next; a
remote mixer changes the mix and runs the transport, nothing else; the remote screen is the
FreeDAWm desktop app.

## The goal

Mix a session from a PC that is not the one running it and plugged into the hardware: see the
faders, pan, plugins and meters, hear the master in real time, change effect settings and write
automation, all from the other PC (the stage PC, another room, later from home).

## Shape

- **The host stays the source of truth.** The PC running the session (the booth) owns the
  document, the audio engine, the plugins and the automation writer. Nothing about recording or
  playback changes there.
- **The remote PC holds a copy.** It runs the normal FreeDAWm UI on a copy of the host's session
  with no audio: its own engine is swapped for a mirror engine while mixing, and its own session
  (document, history, settings) is put aside untouched and comes back when mixing stops.
- **Everything is a command, so everything goes through one place.** `Engine::relay` decides for
  every command on the remote PC (`soundcraft_net::mixer::route`):
  - mix and automation changes (`mix.*`, `automation.*`): run here at once, so the fader follows
    the hand, and are sent to the host;
  - the transport and Undo/Redo (`transport.*`, `edit.undo`, `edit.redo`): only the host runs them;
  - views, selections and queries: here only;
  - anything else that edits or replaces the session (`session.*`, `file.*`, `process.*`, any
    other undoable command): refused, with a reason.
  The host checks the same list for a mixer connection (`host_allows`), so a modified client
  can't do more.
- **Gestures stay gestures.** A fader drag on the remote PC runs `execute_merged`; the relay
  sends its key and the host merges the drag into one undo step as a local drag is.
- **Automation is written by the host.** Its writer counts any command that changes an automatable
  value as a touch, so remote moves write in Touch, Latch and Write as local moves do. A control
  held still on the remote PC (a fader held in Touch) sends `automation.touch` every 200 ms; a
  touch lasts half a second, so a dropped link lets go by itself.

## The wire

The existing LAN channel (port 48100, paired devices, JSON lines) with `hello {role: "mixer"}`.
The host then pushes, from a thread per mixer:

| Event | When | What |
|---|---|---|
| `mix.rest` | it changed | the document without its tracks |
| `mix.track` | that track changed | one track, by id |
| `mix.order` | tracks added, removed or moved | the track ids in order |
| `mix.rev` | after the pieces | the pieces make a whole: the remote PC may rebuild |
| `mix.meters` | ~30 a second | each track's and the main's peaks (and gain reduction), playing, recording, the playhead |
| `stream` | while subscribed and the host plays | the remote mix stream (Opus, as for remote performers) |

The host cuts the session into pieces at most 20 times a second when its revision changed, and
sends only the pieces whose JSON changed (a fader drag sends one track). The remote PC sends
`engine.execute {command, params, merge?}` and `stream.subscribe`.

**No fighting over a fader.** The remote PC puts the host's document in place only when none of
its own commands is waiting for an answer and 120 ms have passed since the last answer, so a
piece the host sent before it ran our change never pulls a fader back.

**What you see moves with what you hear.** The meters and the playhead are delayed by the
stream's jitter buffer, so they line up with the audio.

**Nothing renders on the remote PC.** Its player gets an empty session at the host's rate; it
plays the host's mix stream.

**This PC's own work is safe.** A relayed document is never "modified", autosave leaves this
PC's own autosave alone while mixing, and quitting (or a graphics reset) puts this PC's own
session back first, so the usual save prompt and autosave apply to it.

## Built (step 1)

- `soundcraft_engine::Engine::relay`, `Relay`, `Engine::mirror_session`.
- `soundcraft_net::mixer`: `route`, `host_allows`, the pieces, `MeterFrame`, `MixerClient`.
- The hub's mixer role, the pieces store and pusher, `publish_mix`, `publish_meters`, mixers
  allowed to hear the stream; `BoothGlue::publish_mix`; the control channel's `merge`.
- The app: `remote_mix` (start, stop, the copy, the meters and transport, held controls), Setup ›
  Network › *Mix this booth from here* (the booth picked and paired above), the banner (what is
  mixed, the link, *Hear its mix*, *Stop mixing*), `net.remote_mix`.
- Tests: the relay (engine), the routing and the pieces (net), a remote PC mixing a host over a
  loopback port, a mixer refused anything else, an unpaired PC refused (net), and a whole app
  mixing a booth and getting its own session back (UI).

## Next

1. Over the internet: the mixer link through iroh as the follower's is (`Internet::bridge_to`),
   with the stream at a lower rate and the latency shown.
2. Automation timing over a long link: stamp each remote move with the timeline position the
   remote PC was hearing, and have the host write it there instead of now.
3. The stream while stopped (auditioning a plugin change with the transport stopped), and a
   lossless stream on the LAN.
4. Third-party plugin windows: a generic parameter editor on the remote PC (names and ranges
   are already known for automation lanes); built-in editors already work on the copy.
5. A mixer pairing kind, so a device paired as a mixer can never be anything more.
6. Control surfaces (the X32 over OSC, Mackie Control/HUI) sending the same commands, so a
   surface on the remote PC mixes the host too.
