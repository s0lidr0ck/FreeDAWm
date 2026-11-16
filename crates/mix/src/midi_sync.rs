//! MIDI clock and MIDI timecode out: the session's tempo and position sent to the output port
//! chosen in Setup › MIDI Beat Clock (`setup.midi_beat_clock`: `enabled`, `destination`, `mtc`,
//! `offset`), so a drum machine or sequencer follows FreeDAWm.
//!
//! - **Clock** (when `enabled`): 24 Timing Clock bytes (0xF8) per quarter note, each at the sample
//!   the tempo map puts it on. Play sends Song Position Pointer (the sixteenth notes from the
//!   session start) then Start (0xFA) at the start, Continue (0xFB) elsewhere; Stop sends Stop
//!   (0xFC). A jump while playing (a loop's wrap, a locate) sends Stop, the new Song Position
//!   Pointer and Continue.
//! - **MTC** (when `mtc`): quarter-frame messages (0xF1), four per frame at the session's frame
//!   rate, the eight of a sequence carrying the timecode (from the session's timecode start) of
//!   the frame its first one began on; 24, 25, 29.97 drop and 30 are MTC's own rates, others go out
//!   as the nearest of them by their count. Full-frame SysEx on a locate is not sent (the output
//!   ring carries 3-byte messages); a receiver locks on within two frames.
//! - Each message is stamped with the time the audio beside it is heard, as External MIDI's are
//!   (`external_midi::OutHub`), plus `offset` samples (negative: earlier).
//!
//! [`SyncOut::block`] runs on the audio thread once per block: no allocation, no locks.

use crate::external_midi;
use soundcraft_model::Session;
use soundcraft_time::{FrameRate, Samples, TICKS_PER_QUARTER, Timecode};

/// Where the sync goes this block (port keys, 0: off) and how much later than the audio.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncCfg {
    pub clock: u64,
    pub mtc: u64,
    pub offset: Samples,
}

impl SyncCfg {
    /// The session's settings (`midi.beat_clock`, `midi.beat_clock.mtc`, the destination port's
    /// name, the offset). Allocation-free.
    pub fn of(s: &Session) -> SyncCfg {
        const DEST: &str = "midi.beat_clock.destination=";
        let port = s.edit.flags.iter().find_map(|f| f.strip_prefix(DEST)).map_or(0, external_midi::port_key);
        let off = s.edit.value("midi.beat_clock.offset", 0.0);
        SyncCfg {
            clock: if s.edit.flag("midi.beat_clock") { port } else { 0 },
            mtc: if s.edit.flag("midi.beat_clock.mtc") { port } else { 0 },
            offset: if off.is_finite() { off.clamp(-1e7, 1e7) as Samples } else { 0 },
        }
    }

    pub fn is_off(&self) -> bool {
        self.clock == 0 && self.mtc == 0
    }
}

/// Which stream a message belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Clock,
    Mtc,
}

/// The clock and timecode generator (see the module docs). One per audio stream.
#[derive(Debug, Clone, Default)]
pub struct SyncOut {
    playing: bool,
    /// Where the last block ended (a block starting elsewhere is a jump).
    end: Samples,
    /// The next clock pulse's index (24 per quarter from the session start).
    pulse: i64,
    /// The next quarter frame's index (four per frame from the session start).
    quarter: i64,
}

impl SyncOut {
    pub fn new() -> SyncOut {
        SyncOut::default()
    }

    /// The block `[pos, pos + frames)` (`playing`: the transport rolls): each message to send,
    /// with its offset in the block, through `emit`. Nothing when `cfg` is off.
    pub fn block(&mut self, s: &Session, cfg: SyncCfg, pos: Samples, frames: usize, playing: bool, mut emit: impl FnMut(Stream, usize, [u8; 3])) {
        let end = pos.saturating_add(frames as Samples);
        let jumped = self.playing && playing && pos != self.end;
        if cfg.clock != 0 {
            if self.playing && (!playing || jumped) {
                emit(Stream::Clock, 0, [0xFC, 0, 0]);
            }
            if playing && (!self.playing || jumped) {
                // Song Position Pointer: the sixteenth notes before the pulse to come.
                let ticks = s.tempo.samples_to_ticks_f(pos.max(0), s.sample_rate);
                let pulse = (ticks * 24.0 / TICKS_PER_QUARTER as f64).ceil() as i64;
                let pulse = pulse.div_euclid(6).saturating_mul(6).max(0);
                let spp = (pulse / 6).clamp(0, 0x3FFF);
                emit(Stream::Clock, 0, [0xF2, (spp & 0x7F) as u8, ((spp >> 7) & 0x7F) as u8]);
                emit(Stream::Clock, 0, [if pulse == 0 && !jumped { 0xFA } else { 0xFB }, 0, 0]);
                self.pulse = pulse;
            }
            if playing {
                loop {
                    let tick = self.pulse.saturating_mul(TICKS_PER_QUARTER) / 24;
                    let at = s.tempo.tick_to_samples(tick, s.sample_rate);
                    if at >= end {
                        break;
                    }
                    if at >= pos {
                        emit(Stream::Clock, usize::try_from(at - pos).unwrap_or(0), [0xF8, 0, 0]);
                    }
                    self.pulse = self.pulse.saturating_add(1);
                }
            }
        }
        if cfg.mtc != 0 && playing {
            let rate = s.frame_rate;
            let sr = s.sample_rate;
            let tc_pos = |q: i64| -> Samples {
                // Quarter frame `q` from the timecode start: frame q/4 plus a quarter of a frame each.
                let f = q.div_euclid(4);
                let a = rate.samples_at_frame(f, sr);
                let b = rate.samples_at_frame(f.saturating_add(1), sr);
                a.saturating_add((b - a) * q.rem_euclid(4) / 4).saturating_sub(s.timecode_start)
            };
            if !self.playing || jumped {
                // The first sequence starts on the frame under `pos` (a sequence's start is even).
                let f = rate.frames_at(pos.saturating_add(s.timecode_start).max(0), sr);
                self.quarter = f.div_euclid(2).saturating_mul(8);
            }
            loop {
                let at = tc_pos(self.quarter);
                if at >= end {
                    break;
                }
                if at >= pos {
                    let piece = self.quarter.rem_euclid(8);
                    let tc = Timecode::from_frames(self.quarter.div_euclid(8).saturating_mul(2), rate);
                    let nibble = match piece {
                        0 => tc.frames & 0x0F,
                        1 => (tc.frames >> 4) & 0x01,
                        2 => tc.seconds & 0x0F,
                        3 => (tc.seconds >> 4) & 0x03,
                        4 => tc.minutes & 0x0F,
                        5 => (tc.minutes >> 4) & 0x03,
                        6 => tc.hours & 0x0F,
                        _ => ((tc.hours >> 4) & 0x01) | (mtc_rate(rate) << 1),
                    };
                    emit(Stream::Mtc, usize::try_from(at - pos).unwrap_or(0), [0xF1, ((piece as u8) << 4) | (nibble as u8 & 0x0F), 0]);
                }
                self.quarter = self.quarter.saturating_add(1);
            }
        }
        self.playing = playing;
        self.end = end;
    }
}

/// MTC's rate code: 0 24 fps, 1 25, 2 29.97 drop, 3 30.
fn mtc_rate(r: FrameRate) -> i64 {
    match r {
        FrameRate::Fps29_97Drop | FrameRate::Fps59_94Drop => 2,
        _ => match r.nominal() {
            n if n <= 24 => 0,
            25 | 50 | 100 => 1,
            _ => 3,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &Session, sync: &mut SyncOut, cfg: SyncCfg, pos: Samples, frames: usize, playing: bool) -> Vec<(Stream, usize, [u8; 3])> {
        let mut out = Vec::new();
        sync.block(s, cfg, pos, frames, playing, |k, at, m| out.push((k, at, m)));
        out
    }

    const CFG: SyncCfg = SyncCfg { clock: 7, mtc: 0, offset: 0 };

    /// 120 bpm at 48 kHz: a pulse every 1000 samples. Play from the start sends SPP 0 and Start,
    /// then 48 pulses a second at their samples; Stop sends Stop.
    #[test]
    fn clock_pulses_follow_the_tempo_with_start_and_stop() {
        let s = Session::default();
        assert_eq!(s.sample_rate.hz(), 48_000);
        let mut sync = SyncOut::new();
        let mut all = Vec::new();
        for b in 0..(48_000 / 500) {
            all.extend(run(&s, &mut sync, CFG, b * 500, 500, true).into_iter().map(|(_, at, m)| (b * 500 + at as i64, m)));
        }
        assert_eq!(all.first().copied(), Some((0, [0xF2, 0, 0])));
        assert_eq!(all.get(1).copied(), Some((0, [0xFA, 0, 0])));
        let pulses: Vec<i64> = all.iter().filter(|(_, m)| m[0] == 0xF8).map(|(at, _)| *at).collect();
        assert_eq!(pulses.len(), 48);
        assert!(pulses.iter().enumerate().all(|(k, at)| *at == k as i64 * 1000), "{pulses:?}");
        let stop = run(&s, &mut sync, CFG, 48_000, 500, false);
        assert_eq!(stop, vec![(Stream::Clock, 0, [0xFC, 0, 0])]);
        assert!(run(&s, &mut sync, CFG, 48_000, 500, false).is_empty(), "stopped: nothing");
    }

    /// Play from bar 2 sends its Song Position (16 sixteenths) and Continue; a jump back (a loop)
    /// sends Stop, the new position and Continue; off sends nothing.
    #[test]
    fn continue_from_elsewhere_and_a_jump_resend_the_position() {
        let s = Session::default();
        let mut sync = SyncOut::new();
        let bar2 = 4 * 24_000;
        let out = run(&s, &mut sync, CFG, bar2, 512, true);
        assert_eq!(out.get(..2).map(<[_]>::to_vec), Some(vec![(Stream::Clock, 0, [0xF2, 16, 0]), (Stream::Clock, 0, [0xFB, 0, 0])]));
        assert_eq!(out.get(2), Some(&(Stream::Clock, 0, [0xF8, 0, 0])));
        let back = run(&s, &mut sync, CFG, 0, 512, true);
        assert_eq!(
            back.get(..3).map(<[_]>::to_vec),
            Some(vec![(Stream::Clock, 0, [0xFC, 0, 0]), (Stream::Clock, 0, [0xF2, 0, 0]), (Stream::Clock, 0, [0xFB, 0, 0])])
        );
        let mut off = SyncOut::new();
        assert!(run(&s, &mut off, SyncCfg::default(), 0, 512, true).is_empty());
    }

    /// The settings: clock and MTC to the destination port when each is on, nothing without a port.
    #[test]
    fn the_settings_choose_the_streams_and_the_port() {
        let mut s = Session::default();
        assert!(SyncCfg::of(&s).is_off());
        s.edit.set_flag("midi.beat_clock", true);
        assert!(SyncCfg::of(&s).is_off(), "no port chosen");
        s.edit.flags.insert("midi.beat_clock.destination=Drum Machine".into());
        let cfg = SyncCfg::of(&s);
        assert_eq!((cfg.clock, cfg.mtc), (external_midi::port_key("Drum Machine"), 0));
        s.edit.set_flag("midi.beat_clock.mtc", true);
        assert_eq!(SyncCfg::of(&s).mtc, external_midi::port_key("Drum Machine"));
    }

    /// MTC at 25 fps: four quarter frames a frame (every 480 samples at 48 kHz), the eight of a
    /// sequence carrying the time of its first frame, the rate code in the last.
    #[test]
    fn mtc_quarter_frames_carry_the_timecode() {
        let s = Session { frame_rate: FrameRate::Fps25, ..Session::default() };
        let cfg = SyncCfg { clock: 0, mtc: 9, offset: 0 };
        let mut sync = SyncOut::new();
        // From 1 s in: frame 25, a sequence starting on frame 24 is under way; the first sent
        // starts on frame 26 (00:00:01:01) at 1 s + 1 frame.
        let start = 48_000;
        let mut all = Vec::new();
        for b in 0..20 {
            let pos = start + b * 480;
            all.extend(run(&s, &mut sync, cfg, pos, 480, true).into_iter().map(|(_, at, m)| (pos + at as i64, m)));
        }
        assert!(all.iter().all(|(_, m)| m[0] == 0xF1));
        let gaps: Vec<i64> = all.windows(2).map(|w| w[1].0 - w[0].0).collect();
        assert!(gaps.iter().all(|g| *g == 480), "{gaps:?}");
        // The first whole sequence starts on frame 26 (two frames a sequence): 00:00:01:01.
        let (at, _) = *all.iter().find(|(_, m)| m[1] >> 4 == 0).unwrap();
        assert_eq!(at, 48_000 + 480 * 4, "a frame after the start");
        let first8: Vec<u8> = all.iter().skip_while(|(_, m)| m[1] >> 4 != 0).take(8).map(|(_, m)| m[1]).collect();
        // Piece n in the high nibble; frames 1, seconds 1, minutes and hours 0, rate code 1 (25 fps).
        assert_eq!(first8, vec![0x01, 0x10, 0x21, 0x30, 0x40, 0x50, 0x60, 0x70 | (1 << 1)]);
    }
}
