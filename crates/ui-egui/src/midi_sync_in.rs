//! Sync with other MIDI gear from the UI thread: following MIDI clock or MIDI timecode that comes
//! in (Setup › MIDI Input Devices › Outputs › Sync in), and MIDI Machine Control out (Sync out ›
//! MMC), both set with `setup.midi_beat_clock`.
//!
//! - **Clock in**: Start plays from the session start, Continue from the Song Position Pointer
//!   last sent (or where the playhead is), Stop stops; a Song Position Pointer while stopped
//!   locates. The clock's tempo is measured (`soundcraft_playback::sync_in::SyncIn::tempo`) and
//!   shown; FreeDAWm plays at its own tempo map, it does not bend to the clock.
//! - **MTC in**: running timecode locates to its position (less the session's timecode start)
//!   and plays; a jump of more than two frames locates again; when it stops coming, Stop. A
//!   full-frame message locates.
//! - **MMC out**: Play, Stop, Record Strobe and Record Exit as the transport does them, and a
//!   Locate (with the timecode of the playhead) when it moves while stopped, to the Sync out port.
//!   With MTC out on, such a locate also sends MTC's full-frame message there.

use crate::SoundApp;
use serde_json::json;
use soundcraft_model::Session;
use soundcraft_playback::sync_in::{SyncEvent, SyncIn};
use soundcraft_time::{Samples, TICKS_PER_QUARTER, Timecode};

/// How the session follows sync coming in (`midi.sync_in`: off, clock or mtc) and from which
/// port (`midi.sync_in.source`, any when unset).
pub fn sync_in_mode(s: &Session) -> (&str, Option<&str>) {
    let text = |k: &str| s.edit.flags.iter().find_map(|f| f.strip_prefix(k)).filter(|v| !v.is_empty());
    (text("midi.sync_in=").unwrap_or("off"), text("midi.sync_in.source="))
}

/// What the UI keeps between frames.
#[derive(Debug, Default)]
pub struct SyncState {
    pub follower: SyncIn,
    /// Where Continue plays from (the last Song Position Pointer), samples.
    continue_at: Option<Samples>,
    /// The transport as MMC out last told it: playing, recording, position.
    mmc_last: Option<(bool, bool, Samples)>,
}

/// A Song Position Pointer's sixteenths as a position.
pub fn spp_position(s: &Session, sixteenths: u32) -> Samples {
    s.tempo.tick_to_samples(i64::from(sixteenths) * TICKS_PER_QUARTER / 4, s.sample_rate)
}

/// A timecode coming in as a position (the session's timecode start is 0); running MTC is two
/// frames old when its last quarter frame comes.
pub fn mtc_position(s: &Session, tc: Timecode, running: bool) -> Samples {
    let tc = if running { Timecode::from_frames(tc.to_frames(s.frame_rate).saturating_add(2), s.frame_rate) } else { tc };
    tc.to_samples(s.frame_rate, s.sample_rate).saturating_sub(s.timecode_start)
}

/// MMC out: the messages to send for the transport going from `prev` to `now` (playing,
/// recording, position).
pub fn mmc_messages(s: &Session, prev: Option<(bool, bool, Samples)>, now: (bool, bool, Samples)) -> Vec<Vec<u8>> {
    let cmd = |c: u8| vec![0xF0, 0x7F, 0x7F, 0x06, c, 0xF7];
    let Some((was_playing, was_recording, was_at)) = prev else { return Vec::new() };
    let (playing, recording, at) = now;
    let mut out = Vec::new();
    if recording && !was_recording {
        out.push(cmd(0x06));
    } else if !recording && was_recording && playing {
        out.push(cmd(0x07));
    }
    if playing && !was_playing && !recording {
        out.push(cmd(0x02));
    } else if !playing && was_playing {
        out.push(cmd(0x01));
    } else if !playing && at != was_at {
        let tc = Timecode::from_samples(at.saturating_add(s.timecode_start).max(0), s.frame_rate, s.sample_rate);
        let rate: u8 = match s.frame_rate.nominal() {
            24 => 0,
            25 => 1,
            _ if s.frame_rate.is_drop() => 2,
            _ => 3,
        };
        let b = |v: i64, max: i64| v.clamp(0, max) as u8;
        out.push(vec![
            0xF0,
            0x7F,
            0x7F,
            0x06,
            0x44,
            0x06,
            0x01,
            b(tc.hours, 23) | (rate << 5),
            b(tc.minutes, 59),
            b(tc.seconds, 59),
            b(tc.frames, 29),
            0,
            0xF7,
        ]);
    }
    out
}

/// MTC out's full-frame message for `at` (sent on a locate while stopped, so a receiver is
/// there before the quarter frames start).
pub fn full_frame(s: &Session, at: Samples) -> Vec<u8> {
    let tc = Timecode::from_samples(at.saturating_add(s.timecode_start).max(0), s.frame_rate, s.sample_rate);
    let rate: u8 = match s.frame_rate.nominal() {
        24 => 0,
        25 => 1,
        _ if s.frame_rate.is_drop() => 2,
        _ => 3,
    };
    let b = |v: i64, max: i64| v.clamp(0, max) as u8;
    vec![0xF0, 0x7F, 0x7F, 0x01, 0x01, b(tc.hours, 23) | (rate << 5), b(tc.minutes, 59), b(tc.seconds, 59), b(tc.frames, 29), 0xF7]
}

/// The name of the input port in `slot`.
fn port_name(app: &SoundApp, slot: u8) -> Option<String> {
    use soundcraft_playback::midi_in::{TYPING_NAME, TYPING_SLOT};
    if usize::from(slot) == TYPING_SLOT {
        return Some(TYPING_NAME.to_string());
    }
    app.midi_inputs.as_ref()?.ports().into_iter().find(|p| p.slot == usize::from(slot)).map(|p| p.name)
}

/// Once per frame: sync in followed, MMC out sent.
pub fn tick(app: &mut SoundApp) {
    let (mode, source) = {
        let (m, src) = sync_in_mode(app.engine.session());
        (m.to_string(), src.map(str::to_string))
    };
    let mut events = Vec::new();
    if let Some(hub) = app.midi_hub.clone() {
        hub.set_sync_in(mode != "off");
        let mut msgs = Vec::new();
        hub.take_sync(&mut msgs);
        for m in msgs {
            if source.as_deref().is_some_and(|src| port_name(app, m.slot).as_deref() != Some(src)) {
                continue;
            }
            let b = m.bytes();
            let kind_ok = match mode.as_str() {
                "clock" => !matches!(b.first(), Some(0xF1 | 0xF0)),
                "mtc" => matches!(b.first(), Some(0xF1 | 0xF0)),
                _ => false,
            };
            if kind_ok {
                app.sync.follower.feed(m.at, b, &mut events);
            }
        }
        if mode == "mtc" {
            app.sync.follower.poll(hub.now_us(), &mut events);
        }
    }
    for e in events {
        apply(app, e);
    }
    mmc_out(app);
}

/// Does what one sync event asks.
pub fn apply(app: &mut SoundApp, e: SyncEvent) {
    let playing = app.is_playing();
    let run = |app: &mut SoundApp, id: &str, p: serde_json::Value| {
        if let Err(err) = app.run(id, p) {
            app.ui.status = err;
        }
    };
    match e {
        SyncEvent::Start => {
            app.sync.continue_at = None;
            run(app, "transport.locate", json!({"at": 0}));
            locate_selection(app, 0);
            if !playing {
                run(app, "transport.play", json!({}));
            }
        }
        SyncEvent::Continue => {
            if let Some(at) = app.sync.continue_at.take() {
                run(app, "transport.locate", json!({"at": at}));
                locate_selection(app, at);
            }
            if !playing {
                run(app, "transport.play", json!({}));
            }
        }
        SyncEvent::Stop => {
            if playing {
                run(app, "transport.stop", json!({}));
            }
        }
        SyncEvent::Position(sixteenths) => {
            let at = spp_position(app.engine.session(), sixteenths);
            app.sync.continue_at = Some(at);
            if !playing {
                run(app, "transport.locate", json!({"at": at}));
                locate_selection(app, at);
            }
        }
        SyncEvent::Timecode { tc, running } => {
            let s = app.engine.session();
            let at = mtc_position(s, tc, running);
            let frame = s.frame_rate.samples_at_frame(1, s.sample_rate).max(1);
            let off = (app.position() - at).abs();
            if !running || !playing || off > 2 * frame {
                run(app, "transport.locate", json!({"at": at}));
                if !playing {
                    locate_selection(app, at);
                }
            }
            if running && !playing {
                run(app, "transport.play", json!({}));
            }
        }
    }
    if let Some(bpm) = app.sync.follower.tempo().filter(|_| matches!(e, SyncEvent::Start | SyncEvent::Continue)) {
        app.ui.status = format!("Following MIDI clock ({bpm:.1} bpm)");
    }
}

/// Stopped, Play starts at the insertion point: a locate puts it there.
fn locate_selection(app: &mut SoundApp, at: Samples) {
    app.engine.session_mut().edit.selection = soundcraft_time::Range::point(at);
}

fn mmc_out(app: &mut SoundApp) {
    let s = app.engine.session();
    let now = (app.is_playing(), app.engine.transport.recording, app.engine.transport.position);
    let (mmc, mtc) = (s.edit.flag("midi.mmc_out"), s.edit.flag("midi.beat_clock.mtc"));
    if !mmc && !mtc {
        app.sync.mmc_last = None;
        return;
    }
    let mut msgs = if mmc { mmc_messages(s, app.sync.mmc_last, now) } else { Vec::new() };
    // MTC out: a locate while stopped sends the full frame of the new position.
    if mtc && !now.0 && app.sync.mmc_last.is_some_and(|(p, _, at)| !p && at != now.2) {
        msgs.push(full_frame(s, now.2));
    }
    app.sync.mmc_last = Some(now);
    let Some(dest) = s.edit.flags.iter().find_map(|f| f.strip_prefix("midi.beat_clock.destination=")).map(str::to_string) else { return };
    if let Some(outs) = &app.midi_outputs {
        for m in msgs {
            outs.send_now(&dest, &m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> SoundApp {
        SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default())
    }

    #[test]
    fn song_position_and_timecode_become_positions() {
        let a = app();
        let s = a.engine.session();
        // 120 bpm at 48 kHz: a sixteenth is 6,000 samples.
        assert_eq!(spp_position(s, 16), 96_000);
        let tc = Timecode { negative: false, hours: 0, minutes: 0, seconds: 2, frames: 0 };
        assert_eq!(mtc_position(s, tc, false), 96_000);
        let fr = s.frame_rate.samples_at_frame(1, s.sample_rate);
        assert_eq!(mtc_position(s, tc, true), 96_000 + 2 * fr, "running MTC is two frames old");
    }

    #[test]
    fn clock_in_starts_locates_and_stops_the_transport() {
        let mut a = app();
        a.run("setup.midi_beat_clock", json!({"sync_in": "clock"})).unwrap();
        assert_eq!(sync_in_mode(a.engine.session()).0, "clock");
        apply(&mut a, SyncEvent::Position(16));
        assert_eq!(a.engine.session().edit.selection.start, 96_000, "stopped: located");
        apply(&mut a, SyncEvent::Continue);
        assert_eq!(a.engine.session().edit.playhead, 96_000, "Continue plays from the Song Position");
        apply(&mut a, SyncEvent::Stop);
        apply(&mut a, SyncEvent::Start);
        assert_eq!(a.engine.session().edit.selection.start, 0, "Start is from the session start");
    }

    #[test]
    fn mmc_out_follows_the_transport() {
        let a = app();
        let s = a.engine.session();
        let cmd = |c: u8| vec![0xF0, 0x7F, 0x7F, 0x06, c, 0xF7];
        assert!(mmc_messages(s, None, (true, false, 0)).is_empty(), "nothing before the first look");
        assert_eq!(mmc_messages(s, Some((false, false, 0)), (true, false, 0)), vec![cmd(0x02)]);
        assert_eq!(mmc_messages(s, Some((true, false, 9)), (false, false, 9)), vec![cmd(0x01)]);
        assert_eq!(mmc_messages(s, Some((false, false, 0)), (true, true, 0)), vec![cmd(0x06)], "Record starts it");
        assert_eq!(mmc_messages(s, Some((true, true, 0)), (true, false, 0)), vec![cmd(0x07)], "punched out");
        let locate = mmc_messages(s, Some((false, false, 0)), (false, false, 96_000));
        assert_eq!(locate.len(), 1);
        assert_eq!(locate[0].get(4..7), Some(&[0x44, 0x06, 0x01][..]), "Locate");
        assert_eq!((locate[0][7] & 0x1F, locate[0].get(8..11)), (0, Some(&[0, 2, 0][..])), "to 00:00:02:00 (the hour byte carries the rate)");
        let ff = full_frame(s, 96_000);
        assert_eq!((ff[5] & 0x1F, ff.get(6..9)), (0, Some(&[0, 2, 0][..])), "MTC's full frame for 00:00:02:00");
    }
}
