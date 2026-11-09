# X32 bus control

Status: step 1 built (console → session). Owner decisions: the X32 is also the audio interface
and runs the live room; FreeDAWm's output feeds the stream from an X32 output; fewer than 30
tracks; the controller is chosen as a mono bus or a stereo pair.

## The idea

The X32's channel faders control the live room, so they can't also control the DAW. One bus's
*sends* can: on the desk, **Sends on Fader** with that bus selected puts every channel's send
level to it on the channel faders. A bus patched to no output makes no sound, so it is only a
controller, and the room's mix (the channel faders, the other buses, the mains) never changes.

- **Mono bus** (any of 1–16): channel `N`'s send level moves the session's `N`th track's volume.
- **Stereo pair** (1–2 … 15–16, linked on the desk): the same, and each channel's send pan moves
  the track's pan (a stereo track's two pans move together, keeping their width where it fits).
  A linked pair's sends and master are the odd bus's on the wire.
- The controller's master fader moves the session's master (switchable).
- Tracks map in order from the top: master, folders and video tracks are skipped; channels 1–32.

## The wire

OSC over UDP to the desk's port 10023. FreeDAWm sends `/xremote` (renewed every 8 s; the desk
forgets a client after 10) and `/info` (the desk's model and firmware), and reads
`/ch/NN/mix/BB/level` (0..1 on the desk's fader law: 0 = -∞, 0.25 = -30, 0.5 = -10, 0.75 = 0,
1 = +10 dB), `/ch/NN/mix/BB/pan` (0..1, stereo pairs) and `/bus/BB/mix/fader`. Everything else
(the channel faders, other buses, mutes) is ignored. Bundles are read; malformed packets dropped.

## In the session

Moves become `mix.volume` / `mix.pan` as a fader drag here does: merged into one undo step per
gesture (moves less than a second apart), and touches for live automation writing. Only each
control's last value in a frame counts. While this PC mixes another remotely, the moves go to
that host like any fader move here.

## The app

Setup › X32 Control… (`window.x32`): the link's state, the address, Mono bus / Stereo pair and
which, the master switch, the steps on the desk, and the channel → track list. `setup.x32`
sets it all for agents. Settings in `network.json` (`x32`).

## Known limits

- Send levels have 161 steps on the desk (about 0.25 dB near 0 dB, coarser low down).
- The mute buttons stay the room's mutes in Sends on Fader: not followed.
- One way: the desk's faders don't follow the session yet.

## Next

1. Both ways: names and colours on the scribble strips, the motor faders following automation
   and changes made here (ignoring the desk's echoes of what we sent).
2. Transport from the desk (user-assignable buttons), select → track selection.
3. Aux ins and FX returns as channels 33–48.
