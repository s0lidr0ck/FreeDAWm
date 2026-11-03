# Audio I/O hardware checklist

> **Last reviewed:** 2026-10-20 · **Last updated:** 2026-10-21 · **Change:** minor (Slim Plugin Editors step; previously major: created) · **Target:** Avid Pro Tools Ultimate 2026.4.1

The owner's pass over the audio I/O work
([design](superpowers/specs/2026-10-20-audio-io-design.md),
[plan](superpowers/plans/2026-10-20-audio-io.md)). The work, and gap G-01 in [gaps.md](gaps.md),
counts as done only when this list passes. You need:
- the **Behringer X32** (X-USB card);
- the **Focusrite Scarlett 2i2**;
- the **Focusrite Scarlett Solo**;
- one short patch cable.

Each line has a tick box, what to do, and the expected result. Tick a line when the result
matches. When it doesn't, write what you saw under the line: the numbers, and the status bar text.

"In time" means within ±1 sample on ASIO, with the same device in and out, once measured. With
different input and output devices on WASAPI it means within about 1 ms (best effort).

## 1. Machine setup (done on this machine)

- [x] `winget install LLVM.LLVM`. Expected: `C:\Program Files\LLVM\bin\libclang.dll` exists. *(Done 2026-10-20: LLVM 23.1.4.)*
- [x] Steinberg's ASIO SDK unpacked into `C:\projects\A18\FireStation\DAW\.asio-sdk` (git-ignored). Expected: that folder holds `common\asio.h`. *(Done 2026-10-20.)*
- [x] `%USERPROFILE%\.cargo\config.toml` has, under `[env]`, `LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'` and `CPAL_ASIO_DIR = 'C:\projects\A18\FireStation\DAW\.asio-sdk'`. Expected: both lines present. `CPAL_ASIO_DIR` is **required**: the driver-latency shim (`soundcraft-asio-latency`) compiles against the SDK headers there. *(Done 2026-10-20.)*
- [ ] After everything has merged: `cargo xtask build-asio` from the repo root. Expected: it ends with `ASIO build ready: …\target\release\freedawm.exe`. If something is missing it lists each missing prerequisite and builds nothing.
- [ ] Start `freedawm.exe --demo`, then open Setup › Playback Engine. Expected: "ASIO" is in the audio system list, next to "WASAPI".

## 2. Before each interface

- [ ] Close every other program that uses the interface. Connect the interface and start `freedawm.exe --demo`. Expected: the demo session plays on the default device.
- [ ] Setup › Playback Engine: system **ASIO**, the interface's driver, rate **48000**, buffer **96**. Expected: the window shows "Mix block: 96 samples (the device buffer)". If the driver forces another size, set 96 in the driver's own panel and choose the device again. The window always shows what the driver actually gives.

## 3. Behringer X32

### X32, once

- [ ] Open a new session. Open Setup › I/O **before arming or monitoring anything**. Expected: the Inputs heading says "Input device not open", and the button reads "Add In 1–8". A track's input menu offers In 1 … In 8. The input device opens only on the first arm or monitor.
- [ ] Arm one track, then look at I/O Setup again. Expected: the heading shows 32 input channels. "Add In 1–32" adds In 1 … In 32 as **one** undo step.
- [ ] Rename two inputs ("Kick Mic" on ch 1, "Vox" on ch 2). Make "Pad L/R" stereo on ch 3. Expected: the track input menus list those names in place of In 1–8, and the current one is highlighted.
- [ ] Try to name an input "Main". Expected: refused with "`Main` is a reserved route name", and the old name comes back.
- [ ] Create the stereo output "Out 5-6" on ch 5. Route one track to it and play. Expected: that track is heard **only** on X32 USB returns 5–6 (on the console), not on the main mix.
- [ ] Route a bus to "Out 5-6" and send a track to the bus. Expected: the bus is heard only on 5–6.
- [ ] Save the session as `x32-io.fdawm` (it's needed for the 2i2 test below).
- [ ] Record offset memory: measure at 96 (the Loopback Measure line under "X32 at 96 samples") and note N96. Switch the buffer to 128. Expected: "not measured at this buffer" in yellow. Measure and note N128. Switch back to 96. Expected: "(measured)" in green and N96 again, with no new measurement.
- [ ] Quit and restart, at 96. Expected: N96 is still "(measured)". Offsets are kept per interface, buffer and rate, with the app preferences.
- [ ] Measure **Cancel**: press Measure, then Cancel while it spins. Expected: the spinner stops, the previous value stays, and Measure can be pressed again at once.
- [ ] Measure **timeout**: press Measure and pull the USB cable while it runs. Expected: within about 5 s an error shows (e.g. "Measure timed out: the audio output stopped", or a device error). No hang, no crash, and the previous value stays. Reconnect, choose the device again, then Measure. Expected: it works.
- [ ] Measure with nothing patched back. Expected: "No click came back on input 1: patch output 1 to it" (with your channel numbers), and the previous value stays.
- [ ] Measure while the transport plays. Expected: refused with "stop the transport first".
- [ ] **Half speed:** press Shift+Space (half-speed playback), then Record. Expected: the status reads "Cannot record: Recording needs full speed: at half speed the take would lose input. Stop, then record." Nothing records.
- [ ] Start recording at full speed, then press Shift+Space. Expected: "Half speed is not available while recording", and the take carries on at full speed.
- [ ] If the console clocks the X32 at 44.1 kHz with a 48 kHz session: record. Expected: the status warns "Device runs at 44.1 kHz, session is 48 kHz: takes are resampled and may drift", and recording goes ahead. Skip this line if you can't change the console's clock.
- [ ] **Plugin editors:** open a FireStation module on a track. Expected: its VST-style editor opens by default. Untick View › Mix Window Views › Plugin Editors as in the VST. Expected: the narrow drawer opens instead. Tick it again. Expected: back to the VST editor. Open a Comp and tick View › Mix Window Views › Slim Plugin Editors. Expected: Comp's editor at the narrow drawer's width; untick it to get the full editor back. While a track is monitored at 96, opening and closing editors leaves Underruns at 0.
- [ ] **Scout (one quick pass, it shares the meter taps):** select a monitored track with a plugin and start a Scout capture while stopped. Play into the input for a few seconds, then stop. Expected: the capture shows PRE and POST levels of the live input. Underruns stay 0 and monitoring doesn't break up. Then set a loop and capture it during playback. Expected: one capture of exactly the loop.

### X32 at 96 samples

- [ ] Playback Engine. Expected: "Mix block: 96 samples (the device buffer)". Reported latency shows in and out values that aren't 0 (the ASIO driver's own figures).
- [ ] Press Reset next to Underruns, then loop the demo session for **5 minutes**. Expected: Underruns stays 0, no red "Underruns" in the status bar, no crackle. Note the DSP load: ____ %.
- [ ] **Loopback Measure:** in the X32 routing, patch a USB (card) output back to a card input, e.g. card out 1 → card in 1. In the Measure row pick that output and input, then press Measure 3 times. Expected: the 3 results agree within ±1 sample. The offset shows "(measured)" in green. Values: ____ / ____ / ____.
- [ ] Hover the Record button. Expected: "Record (Cmd+Space) · offset N samples (measured)".
- [ ] **DAW monitoring:** arm an audio track with a plugin (e.g. a FireStation Comp), I button Auto (plain). Play an instrument or speak into it. Expected: you hear the input through the plugin, in time with the click, with no flam you can hear.
- [ ] Click I once: **On** (lit green). Disarm the track. Expected: you still hear the input.
- [ ] Click I again: **Off** (red outline, struck-through I). Arm and record. Expected: the DAW is silent on that track. You hear the X32's own monitoring of the channel instead (direct monitoring).
- [ ] **LLM on:** put a latent plugin (e.g. a maximizer) on the armed track and on the master, then click the transport's LLM key (after Loop). Expected:
  - both tiles show the "LLM" pill;
  - the monitor path gets tighter (less delay against the click);
  - Options › Low Latency Monitoring is ticked;
  - Undo History has no new step.
- [ ] The same latent plugin on an **Instrument** track. Expected: no LLM pill there (LLM covers audio tracks only).
- [ ] **LLM off.** Expected: the pills go, the plugins are heard again, and there is no undo step.
- [ ] **Take alignment:** Track › Create Click Track. Keep the card out → card in loopback. Arm a mono track on that input with I **Off**, record 8 bars, then zoom to samples on a few beats. Expected: each recorded click starts within ±1 sample of the timeline click.
- [ ] The same take with LLM **on** and a latent plugin on another track. Expected: still within ±1 sample.
- [ ] Routing: the track on "Out 5-6" plays at 96. Expected: heard only on 5–6, with no crackle.

### X32 at 128 samples

- [ ] Playback Engine. Expected: "Mix block: 128 samples (the device buffer)". Reported latency shows in and out values that aren't 0.
- [ ] Reset Underruns, then loop the demo session for 5 minutes. Expected: Underruns stays 0, no crackle. DSP load: ____ %.
- [ ] Loopback Measure 3 times. Expected: the results agree within ±1 sample, "(measured)" in green. Values: ____ / ____ / ____.
- [ ] DAW monitoring (I Auto, armed, plugin). Expected: heard, in time.
- [ ] Direct monitoring (I Off). Expected: the DAW is silent on that track; the X32's monitoring is heard.
- [ ] LLM on: "LLM" pills on latent inserts, monitoring tighter. LLM off: restored, no undo step.
- [ ] Take alignment against the click track. Expected: within ±1 sample, with LLM off and on.
- [ ] Routing: the "Out 5-6" track is heard only on 5–6.

## 4. Focusrite Scarlett 2i2

For the loopback, connect output 1 to input 1 with the cable. Turn input 1's gain down, and switch
**Direct Monitor off** while measuring and recording the loopback, or the interface feeds back.

### 2i2, once

- [ ] Open `x32-io.fdawm` on the 2i2 at 96. Expected:
  - in I/O Setup and the route menus, the inputs and outputs past the 2i2's 2 channels show "(not available)" in red (dimmed in the menus);
  - the session plays;
  - nothing crashes;
  - the track routed to "Out 5-6" is silent.
- [ ] Arm a track whose input is a missing X32 channel. Expected: refused with "{track}: input “{name}” is not available on this device (2 inputs)".
- [ ] Record offset memory: measure at 96, switch to 128 (Expected: "not measured at this buffer"), measure, then back to 96. Expected: the 96 value is back, "(measured)". The X32's values are kept separately.
- [ ] Measure Cancel, then the timeout (pull the USB cable mid-measure). Expected: as for the X32: an error within about 5 s, no hang, the previous value kept, and Measure works again after reconnecting.

### 2i2 at 96 samples

- [ ] Playback Engine. Expected: "Mix block: 96 samples (the device buffer)", and the reported latency shows.
- [ ] Reset Underruns, then loop the demo session for 5 minutes. Expected: Underruns stays 0, no crackle. DSP load: ____ %.
- [ ] Loopback Measure (output 1 → input 1 cable) 3 times. Expected: the results agree within ±1 sample, "(measured)". Values: ____ / ____ / ____.
- [ ] DAW monitoring with a plugin on an armed track (I Auto). Expected: audible and in time.
- [ ] Direct monitoring: I Off, then the 2i2's Direct Monitor switch on. Expected: the DAW is silent on that track, and the 2i2's direct monitor is heard.
- [ ] LLM on: "LLM" pills, monitoring tighter. LLM off: restored, no undo step.
- [ ] Take alignment: record the looped-back click against the click track (I Off). Expected: within ±1 sample, with LLM off and on.

### 2i2 at 128 samples

- [ ] Playback Engine. Expected: "Mix block: 128 samples (the device buffer)", and the reported latency shows.
- [ ] Reset Underruns, then loop the demo session for 5 minutes. Expected: Underruns stays 0, no crackle. DSP load: ____ %.
- [ ] Loopback Measure 3 times. Expected: the results agree within ±1 sample. Values: ____ / ____ / ____.
- [ ] DAW monitoring (I Auto). Expected: audible and in time.
- [ ] Direct monitoring (I Off + Direct Monitor). Expected: the DAW is silent on that track, and the interface is heard.
- [ ] LLM on and off. Expected: the pills come and go; no undo step.
- [ ] Take alignment. Expected: within ±1 sample, with LLM off and on.

### 2i2 on WASAPI with a different input device (best effort)

- [ ] Playback Engine: system WASAPI, output the 2i2, input another device (e.g. the laptop's microphone), at 48000. Expected: it opens. The mix block is the WASAPI buffer.
- [ ] Measure with a loopback you can make between the two devices (e.g. a 2i2 output into the other device's line input). Expected: a result, "(measured)".
- [ ] Quit and restart, then open Playback Engine **before** arming or monitoring. Expected (**known limit**): it says "not measured at this buffer" until the input opens. Arm a track, and the measured value shows again.
- [ ] Take alignment against the click track. Expected: within about **1 ms** (48 samples).
- [ ] Reset Underruns, then play for 5 minutes. Expected: Underruns stays 0, no crackle.

## 5. Focusrite Scarlett Solo (one smoke pass at 128)

- [ ] ASIO, the Solo's driver, 48000, buffer 128. Expected: "Mix block: 128 samples (the device buffer)".
- [ ] Reset Underruns, then play the demo session for 2 minutes. Expected: Underruns 0, no crackle.
- [ ] Loopback Measure (output 1 → input, Direct Monitor off). Expected: a stable number over 2 runs, "(measured)".
- [ ] DAW monitoring on an armed track with a plugin. Expected: audible and in time. I Off. Expected: the DAW is silent on that track.
- [ ] LLM on, then off. Expected: the pills come and go; no undo step.
- [ ] Take alignment against the click track. Expected: within ±1 sample.

## 6. Sign-off

- [ ] Every line above ticked, or its failure written down and reported. Expected: the owner says G-01 is closed.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-21 | minor | Plugin editors step: Slim Plugin Editors on a Comp |
| 2026-10-20 | major | Created: the owner's pass over audio I/O on the X32, 2i2 (96 and 128) and Solo (128) |
