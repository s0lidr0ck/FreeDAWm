# Mixing a live stream from home

You run a live stream from a studio PC (a church service, a concert, a podcast). FreeDAWm on
that PC mixes the live inputs and sends the mix out of a spare hardware output to the streaming
computer or encoder. With **File › Connect to a Studio…** you open that studio's mixer on another
PC running FreeDAWm, at home or in another room, hear its master, and change the mix while the
event goes on. Nothing you do there can stop or disturb the studio's sound.

## On the studio PC (once)

1. **Feed the stream from a spare output.** In the Mix window, give the master (or a bus made
   for the stream) a hardware output that goes to the streaming computer, e.g. *Out 3-4* cabled
   to its capture input. The live inputs play through their strips with Input Monitoring on;
   the transport can stay stopped the whole time.
2. **Let the network in.** Setup › Network… › *Allow control from this network*. On the same
   network that is all. For home, also tick *Let a performer join this PC over the internet* and
   press **Invite…** (one person, 30 minutes; or Reusable for a day or a week) and send the code
   to the person mixing (email, chat).
3. Optional: set a **Booth PIN** (Setup › Network › Booth PIN) so someone on the same network can
   pair by typing it, with nobody at the studio reading out a code.
4. Keep the studio awake: FreeDAWm already does while remotes are connected or live inputs are
   monitored.

## At home

1. **File › Connect to a Studio…** The dialog shows:
   - *Studios on this network*: click **Connect**. The first time, the studio shows a 4-digit
     code (or waits for its booth PIN): type it in the box and press Connect.
   - *Studios you have mixed before*: **Reconnect** works with one click, no code or invite.
   - *Paste the PIN, code or invite*: paste the `freedawm-invite:` code from the studio to join
     over the internet, or type the studio's address (`192.168.1.20:48100`) if it is not found.
2. The **Remote Mix** window opens (its own window, with a purple banner): "Mixing *Studio PC*
   remotely · *the session's name*". Your own session is untouched and stays in the main window.
3. **Hear the studio.** Its master plays through FreeDAWm's own output device (Setup › Playback
   Engine), mixed into your session's output, even while the studio is stopped. *Listen* sets
   how loud it plays here and *Mute* silences it here; neither changes anything at the studio.

## What you can do from home

- **Faders, pan, mute, solo**, by the same commands the studio's own Mix window runs. A drag is
  one undo step on the studio.
- **Send levels**: drag a send left or right (double-click: 0 dB).
- **Insert bypass**: the power key beside each insert.
- **Built-in effects**: click an insert's name. Most built-in effects open their own editor
  (Comp, Amp, Delay, Dist, Filter, Manners, Mod, Pitch, Reverb, Sat, Shale, Sift, Swale, Tape,
  Transient, Twine, Heft, Keel, Lull, Rez, Sprawl), working on the studio's insert. CleanQ,
  CopyQ, LevelUp, Chamber, the analysers (Spectrum, Loudness Meter, Phase Meter, Tuner) and the
  plain built-ins (such as the 7-band EQ) open as a list of their parameters with a slider each.
  Graphs and meters that need the studio's sound stay still here.
- **Third-party plug-ins** (VST3, CLAP, AU): name and bypass only. Their windows and settings
  live on the studio PC.
- **Undo and Redo** act on the studio's history, whoever made the change; the buttons show what
  they will undo, and the first time you are asked to confirm.
- **Mix snapshots**: pick one and **Recall…** (asked every time), or type a name and **Store**.
- **Seen but changed only at the studio**: input monitoring, routing, automation, adding or
  removing plug-ins, anything on the timeline, files and sessions.

## The delay to expect

- A control you move shows at once here and reaches the studio in a round trip plus a studio
  frame: about 20 ms on a local network (measured on one machine), add your internet round trip
  from home.
- What you hear is later: the stream is Opus at 256 kb/s with a jitter buffer of 150 ms that
  grows to 500 ms on a bad connection. Expect roughly 0.2 to 0.6 s behind the room. Judge balance
  by ear, but do not expect to hit a cue on the beat.
- The banner shows the round trip, the stream's buffer and dropouts, and warns when the
  connection is weak.
- Data: about 7 to 10 KB/s for the mixer and meters (32 strips) plus 32 KB/s for the sound.

## When someone else moves the same control

The first hand on a control keeps it until it lets go and half a second more: another remote
PC moving the same fader meanwhile is refused, and its window says who has it. The studio's own
screen is not held back: if the engineer at the studio moves a fader you are dragging, the last
move wins, and your fader shows the studio's value as soon as you let go.

## If the connection drops

- The window greys and says **Reconnecting…**, keeps the last state visible, and tries again
  every couple of seconds. When it is back it takes a fresh picture of the mixer.
- A change you made while it was down, or that was still waiting to go, is **never sent later**:
  the window says how many were not sent ("3 changes were not sent").
- At the studio nothing changes: the mix stays exactly as last set, a fader you were dragging
  stays where it was and its undo step closes. The studio never waits for you.

## On the studio, during the event

Setup › Network › **Remote mixing** lists who is connected, "mixing · last change 12 s ago" once
they change something (a notice says so the first time), with:

- **Disconnect**: ends their connection now and keeps them out until you press *Let back in*
  (or FreeDAWm restarts). What they changed stays.
- **Forget**: ends their connection and unpairs them.
- **Lock remote mixing**: they keep seeing the mixer and hearing the mix, but every change from
  them is refused (their window greys and says so). It applies to the whole session and every
  remote alike, and stays on until you turn it off.

## Not tried yet

Windows and macOS, a real internet connection between two homes, and real audio devices were
not tested for this feature; everything above was tested on one Linux machine with the two PCs
in one process (the network, pairing, encrypted links and invites for real; the audio as data).
