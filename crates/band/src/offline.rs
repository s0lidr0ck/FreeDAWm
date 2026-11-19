//! Offline renders of the band, for listening and for measuring how it plays (not the audio path).
//!
//! [`render`] plays a [`BandSettings`] through a [`BandRuntime`] block by block, as the mix engine does, and keeps
//! every event with its time; [`smf_bytes`] writes those events as a Standard MIDI File, one track per musician.
//! Both allocate freely: they run on a worker or in a test, never on the audio thread.

use crate::band::NUM_MUSICIANS;
use crate::midi_types::{MidiEvent, MidiSink};
use crate::runtime::{BandRuntime, BandSettings, HostTransport};

/// One event the band sent, with where it sounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedEvent {
    /// Milliseconds from the start of the render.
    pub ms: f64,
    /// Quarter notes from the start of the render.
    pub ppq: f64,
    /// Who played it (0..6), -1 when not known.
    pub musician: i8,
    pub status: u8,
    pub d1: u8,
    pub d2: u8,
    /// A keyswitch (not a played note).
    pub keyswitch: bool,
}

impl TimedEvent {
    pub fn is_note_on(&self) -> bool {
        self.status & 0xF0 == 0x90 && self.d2 > 0 && !self.keyswitch
    }
    pub fn is_note_off(&self) -> bool {
        !self.keyswitch && (self.status & 0xF0 == 0x80 || (self.status & 0xF0 == 0x90 && self.d2 == 0))
    }
}

/// How a render is played: tempo, the host's meter, length, rate and block size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderSpec {
    pub bpm: f64,
    pub num: i32,
    pub den: i32,
    /// Quarter notes to play before the transport stops.
    pub quarters: f64,
    pub sample_rate: f64,
    pub block: usize,
}

impl Default for RenderSpec {
    fn default() -> Self {
        RenderSpec { bpm: 120.0, num: 4, den: 4, quarters: 64.0, sample_rate: 48000.0, block: 256 }
    }
}

struct Collect<'a> {
    out: &'a mut Vec<TimedEvent>,
    base_ms: f64,
    base_ppq: f64,
    ms_per_sample: f64,
    ppq_per_sample: f64,
}

impl MidiSink for Collect<'_> {
    fn event(&mut self, e: &MidiEvent) {
        let off = f64::from(e.offset.max(0));
        self.out.push(TimedEvent {
            ms: self.base_ms + off * self.ms_per_sample,
            ppq: self.base_ppq + off * self.ppq_per_sample,
            musician: e.musician,
            status: e.status,
            d1: e.d1,
            d2: e.d2,
            keyswitch: e.keyswitch,
        });
    }
}

/// Plays `settings` from quarter 0 for `spec.quarters`, then stops (the notes still sounding close). Events come
/// back in time order (ties keep the order they were sent).
pub fn render(settings: BandSettings, spec: &RenderSpec) -> Vec<TimedEvent> {
    let sr = spec.sample_rate.max(1.0);
    let block = spec.block.max(1);
    let mut rt = BandRuntime::new(sr, settings);
    let ppq_per_sample = spec.bpm / 60.0 / sr;
    let ms_per_sample = 1000.0 / sr;
    let mut events = Vec::new();
    let mut i: u64 = 0;
    loop {
        let base_samples = i as f64 * block as f64;
        let ppq = base_samples * ppq_per_sample;
        let playing = ppq < spec.quarters;
        let mut sink = Collect { out: &mut events, base_ms: base_samples * ms_per_sample, base_ppq: ppq, ms_per_sample, ppq_per_sample };
        rt.process_block(&HostTransport { playing, bpm: spec.bpm, ppq, num: spec.num, den: spec.den }, block, &mut sink);
        i += 1;
        if !playing {
            break;
        }
    }
    events.sort_by(|a, b| a.ms.total_cmp(&b.ms));
    events
}

fn vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 5];
    let mut n = 0;
    loop {
        buf[n] = (v & 0x7F) as u8;
        n += 1;
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    for k in (0..n).rev() {
        out.push(buf[k] | if k > 0 { 0x80 } else { 0 });
    }
}

fn track(out: &mut Vec<u8>, name: &str, body: &[(u32, [u8; 3], usize)], meta: &[u8]) {
    let mut t = Vec::new();
    vlq(&mut t, 0);
    t.extend_from_slice(&[0xFF, 0x03]);
    vlq(&mut t, name.len() as u32);
    t.extend_from_slice(name.as_bytes());
    t.extend_from_slice(meta);
    let mut last = 0u32;
    for &(tick, bytes, len) in body {
        vlq(&mut t, tick.saturating_sub(last));
        last = tick.max(last);
        t.extend_from_slice(bytes.get(..len).unwrap_or(&bytes));
    }
    vlq(&mut t, 0);
    t.extend_from_slice(&[0xFF, 0x2F, 0x00]);
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&(t.len() as u32).to_be_bytes());
    out.extend_from_slice(&t);
}

/// The events as a format 1 Standard MIDI File at 960 ticks a quarter: a tempo track, then one track per musician
/// (named from `names`), each musician's notes on its own channel (musician k on channel k+1; the drummer on 10
/// when `drums` names it). Keyswitches are left out unless `keyswitches`.
pub fn smf_bytes(events: &[TimedEvent], spec: &RenderSpec, names: &[String; NUM_MUSICIANS], drums: Option<usize>, keyswitches: bool) -> Vec<u8> {
    const TPQ: f64 = 960.0;
    let ms_per_q = 60000.0 / spec.bpm.max(1.0);
    let mut out = Vec::new();
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    let used: Vec<usize> = (0..NUM_MUSICIANS).filter(|&k| events.iter().any(|e| e.musician as usize == k && e.musician >= 0)).collect();
    out.extend_from_slice(&((used.len() + 1) as u16).to_be_bytes());
    out.extend_from_slice(&(TPQ as u16).to_be_bytes());
    let us = (ms_per_q * 1000.0).round() as u32;
    let den_pow = match spec.den {
        8 => 3u8,
        2 => 1,
        _ => 2,
    };
    let mut meta = vec![0x00, 0xFF, 0x51, 0x03, (us >> 16) as u8, (us >> 8) as u8, us as u8];
    meta.extend_from_slice(&[0x00, 0xFF, 0x58, 0x04, spec.num.clamp(1, 32) as u8, den_pow, 24, 8]);
    track(&mut out, "Tempo", &[], &meta);
    for k in used {
        let ch = if drums == Some(k) { 9u8 } else { (k as u8) & 0x0F };
        let mut body: Vec<(u32, [u8; 3], usize)> = Vec::new();
        for e in events.iter().filter(|e| e.musician as usize == k && e.musician >= 0) {
            if e.keyswitch && !keyswitches {
                continue;
            }
            let kind = e.status & 0xF0;
            if kind < 0x80 || kind == 0xF0 {
                continue;
            }
            let len = if kind == 0xC0 || kind == 0xD0 { 2 } else { 3 };
            let tick = (e.ms / ms_per_q * TPQ).round().max(0.0) as u32;
            body.push((tick, [kind | ch, e.d1 & 0x7F, e.d2 & 0x7F], len));
        }
        body.sort_by_key(|b| b.0);
        let name = names.get(k).cloned().unwrap_or_default();
        track(&mut out, &name, &body, &[]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vlq_encodes_like_the_spec() {
        let enc = |v: u32| {
            let mut o = Vec::new();
            vlq(&mut o, v);
            o
        };
        assert_eq!(enc(0), vec![0]);
        assert_eq!(enc(0x7F), vec![0x7F]);
        assert_eq!(enc(0x80), vec![0x81, 0x00]);
        assert_eq!(enc(0x3FFF), vec![0xFF, 0x7F]);
    }
}
