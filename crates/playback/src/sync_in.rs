//! MIDI clock and MIDI timecode in: what a drum machine, sequencer or tape machine sends so that
//! FreeDAWm follows it (the other way round from `soundcraft_mix::midi_sync`).
//!
//! The input ports hand the hub's control ring the sync bytes they get (System Real-Time Start,
//! Continue, Stop and Timing Clock, Song Position Pointer, MTC quarter frames and the MTC
//! full-frame SysEx), stamped on the hub's clock; the UI thread feeds them to a [`SyncIn`], which
//! turns them into [`SyncEvent`]s: the transport to start, continue or stop, where to locate, and
//! the tempo the clock runs at (the average of the last 24 clock intervals, a quarter note's).
//! MTC's eight quarter frames carry a timecode that was current two frames before the last one
//! arrived, so the position reported is that plus two frames; a stream that stops for longer
//! than [`MTC_DROPOUT_US`] stops the transport.
//!
//! Nothing here runs on the audio thread.

use soundcraft_time::Timecode;

/// Clock intervals averaged for the tempo (one quarter note).
const CLOCK_WINDOW: usize = 24;

/// No MTC quarter frame for this long (µs): the timecode stopped.
pub const MTC_DROPOUT_US: u64 = 150_000;

/// What the sync coming in asks of the transport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncEvent {
    /// MIDI Start: play from the session start.
    Start,
    /// MIDI Continue: play from the position last set (Song Position Pointer) or where it is.
    Continue,
    /// MIDI Stop.
    Stop,
    /// Song Position Pointer: sixteenth notes from the session start.
    Position(u32),
    /// MTC (quarter frames or a full frame): the timecode now, and whether it is running (quarter
    /// frames) or a locate (full frame).
    Timecode { tc: Timecode, running: bool },
}

/// The follower (see the module docs).
#[derive(Debug, Clone, Default)]
pub struct SyncIn {
    last_clock: Option<u64>,
    intervals: [u64; CLOCK_WINDOW],
    filled: usize,
    next: usize,
    /// The eight MTC quarter-frame nibbles, and which have come since the last full set.
    pieces: [u8; 8],
    seen: u8,
    /// When the last quarter frame came (µs), 0: none, and whether MTC is running.
    last_qf: u64,
    mtc_running: bool,
    /// The clock is running (Start or Continue came, no Stop since).
    clock_running: bool,
}

/// Whether `bytes` is a message [`SyncIn`] reads (the hub hands only these to it).
pub fn is_sync(bytes: &[u8]) -> bool {
    matches!(bytes, [0xF8 | 0xFA | 0xFB | 0xFC, ..] | [0xF2, _, _, ..] | [0xF1, _, ..] | [0xF0, 0x7F, _, 0x01, 0x01, _, _, _, _, ..])
}

impl SyncIn {
    pub fn new() -> SyncIn {
        SyncIn::default()
    }

    /// One message from a port, stamped `at` (µs); what it asks for goes into `out`.
    pub fn feed(&mut self, at: u64, bytes: &[u8], out: &mut Vec<SyncEvent>) {
        match bytes {
            [0xF8, ..] => self.clock(at),
            [0xFA, ..] => {
                self.clock_running = true;
                self.last_clock = None;
                out.push(SyncEvent::Start);
            }
            [0xFB, ..] => {
                self.clock_running = true;
                self.last_clock = None;
                out.push(SyncEvent::Continue);
            }
            [0xFC, ..] => {
                self.clock_running = false;
                out.push(SyncEvent::Stop);
            }
            [0xF2, lsb, msb, ..] => out.push(SyncEvent::Position(u32::from(lsb & 0x7F) | (u32::from(msb & 0x7F) << 7))),
            [0xF1, data, ..] => {
                if let Some(tc) = self.quarter_frame(at, *data) {
                    out.push(SyncEvent::Timecode { tc, running: true });
                }
            }
            // Full frame: F0 7F <device> 01 01 hr mn sc fr F7 (hour byte: 0rrhhhhh).
            [0xF0, 0x7F, _, 0x01, 0x01, hr, mn, sc, fr, ..] => {
                let tc = Timecode {
                    negative: false,
                    hours: i64::from(hr & 0x1F),
                    minutes: i64::from(mn & 0x3F),
                    seconds: i64::from(sc & 0x3F),
                    frames: i64::from(fr & 0x1F),
                };
                self.seen = 0;
                out.push(SyncEvent::Timecode { tc, running: false });
            }
            _ => {}
        }
    }

    /// Called now and then with the time (µs): MTC that stopped coming stops the transport.
    pub fn poll(&mut self, now: u64, out: &mut Vec<SyncEvent>) {
        if self.mtc_running && now.saturating_sub(self.last_qf) > MTC_DROPOUT_US {
            self.mtc_running = false;
            self.seen = 0;
            out.push(SyncEvent::Stop);
        }
    }

    /// The tempo the clock runs at (beats per minute), once a few clocks came.
    pub fn tempo(&self) -> Option<f64> {
        if self.filled < 6 {
            return None;
        }
        let n = self.filled.min(CLOCK_WINDOW);
        let sum: u64 = self.intervals.iter().take(n).sum();
        let per_clock = sum as f64 / n as f64;
        (per_clock > 0.0).then(|| 60e6 / (per_clock * 24.0))
    }

    /// The clock was started and not stopped.
    pub fn clock_running(&self) -> bool {
        self.clock_running
    }

    fn clock(&mut self, at: u64) {
        if let Some(prev) = self.last_clock {
            let d = at.saturating_sub(prev);
            // A gap of more than a second is not one clock (a pause, a new stream).
            if d > 0 && d < 1_000_000 {
                if let Some(slot) = self.intervals.get_mut(self.next) {
                    *slot = d;
                }
                self.next = (self.next + 1) % CLOCK_WINDOW;
                self.filled = (self.filled + 1).min(CLOCK_WINDOW);
            }
        }
        self.last_clock = Some(at);
    }

    /// A quarter frame: the timecode once its eighth piece completes a set.
    fn quarter_frame(&mut self, at: u64, data: u8) -> Option<Timecode> {
        let piece = usize::from((data >> 4) & 0x07);
        if let Some(p) = self.pieces.get_mut(piece) {
            *p = data & 0x0F;
        }
        self.seen |= 1 << piece;
        self.last_qf = at;
        self.mtc_running = true;
        if piece != 7 || self.seen != 0xFF {
            return None;
        }
        self.seen = 0;
        let v = |lo: usize| self.pieces.get(lo).copied().unwrap_or(0) | (self.pieces.get(lo + 1).copied().unwrap_or(0) << 4);
        let (fr, sc, mn, hr) = (v(0) & 0x1F, v(2) & 0x3F, v(4) & 0x3F, v(6) & 0x1F);
        Some(Timecode { negative: false, hours: i64::from(hr), minutes: i64::from(mn), seconds: i64::from(sc), frames: i64::from(fr) })
    }
}

/// The MTC rate code of a full set's hour byte (0: 24, 1: 25, 2: 29.97 drop, 3: 30), for display.
pub fn mtc_rate_name(code: u8) -> &'static str {
    match code & 0x03 {
        0 => "24",
        1 => "25",
        2 => "29.97 drop",
        _ => "30",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_messages_and_song_position() {
        let mut s = SyncIn::new();
        let mut out = Vec::new();
        for b in [&[0xF2u8, 0x10, 0x01][..], &[0xFB], &[0xFC], &[0xFA]] {
            s.feed(0, b, &mut out);
        }
        assert_eq!(out, vec![SyncEvent::Position(0x10 | (1 << 7)), SyncEvent::Continue, SyncEvent::Stop, SyncEvent::Start]);
        assert!(s.clock_running());
    }

    #[test]
    fn the_clock_gives_its_tempo() {
        let mut s = SyncIn::new();
        let mut out = Vec::new();
        s.feed(0, &[0xFA], &mut out);
        // 125 bpm: a quarter is 480 ms, a clock 20 ms; a little jitter either way.
        let mut at = 1_000u64;
        for k in 0..60u64 {
            s.feed(at, &[0xF8], &mut out);
            at += if k % 2 == 0 { 20_300 } else { 19_700 };
        }
        let bpm = s.tempo().unwrap();
        assert!((bpm - 125.0).abs() < 0.5, "{bpm}");
        assert_eq!(out, vec![SyncEvent::Start], "clocks themselves ask nothing of the transport");
    }

    /// The eight quarter frames of `tc` (25 fps: rate code 1).
    fn quarters(h: u8, m: u8, s: u8, f: u8) -> Vec<[u8; 2]> {
        let hr = h | (1 << 5);
        [f & 0x0F, f >> 4, s & 0x0F, s >> 4, m & 0x0F, m >> 4, hr & 0x0F, hr >> 4]
            .iter()
            .enumerate()
            .map(|(k, n)| [0xF1, ((k as u8) << 4) | n])
            .collect()
    }

    #[test]
    fn mtc_quarter_frames_give_the_timecode_and_a_dropout_stops() {
        let mut s = SyncIn::new();
        let mut out = Vec::new();
        let mut at = 0u64;
        for q in quarters(1, 2, 3, 4) {
            s.feed(at, &q, &mut out);
            at += 10_000;
        }
        let tc = Timecode { negative: false, hours: 1, minutes: 2, seconds: 3, frames: 4 };
        assert_eq!(out, vec![SyncEvent::Timecode { tc, running: true }]);
        // A set begun part way (pieces 4 to 7 only) says nothing.
        out.clear();
        for q in quarters(1, 2, 3, 6).into_iter().skip(4) {
            s.feed(at, &q, &mut out);
        }
        assert!(out.is_empty());
        s.poll(at + 50_000, &mut out);
        assert!(out.is_empty(), "still running");
        s.poll(at + MTC_DROPOUT_US + 1, &mut out);
        assert_eq!(out, vec![SyncEvent::Stop]);
    }

    #[test]
    fn a_full_frame_locates() {
        let mut s = SyncIn::new();
        let mut out = Vec::new();
        let msg = [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x20 | 10, 20, 30, 12, 0xF7];
        assert!(is_sync(&msg) && is_sync(&[0xF8]) && !is_sync(&[0x90, 60, 1]));
        s.feed(0, &msg, &mut out);
        let tc = Timecode { negative: false, hours: 10, minutes: 20, seconds: 30, frames: 12 };
        assert_eq!(out, vec![SyncEvent::Timecode { tc, running: false }]);
    }
}
