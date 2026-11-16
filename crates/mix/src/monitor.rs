//! The control room's audio (see [`soundcraft_model::ControlRoom`]): the listen bus for AFL/PFL
//! solo, the monitor path (the main mix, or the listen bus, onto the active monitor set with its
//! level, dim, mono and mute), the cue mixes' levels and the talkback microphone in the cues.
//!
//! All of it is for live playback only: an offline render (bounce, stems, freeze, commit) never
//! runs it, and it comes after the main meters and the loudness meter, so what a mix measures and
//! prints is the same with a control room or without. In AFL and PFL solo modes a solo does not
//! solo in place: the main mix plays on untouched (in a bounce too) and the soloed tracks are
//! heard on the listen bus, which replaces the mix on the monitor path while any track is soloed.

use crate::{MixEngine, PanSpec, Strip, apply_matrix_span, build_matrix, pan_at, strip_format};
use soundcraft_dsp::db_to_gain;
use soundcraft_model::{ChannelFormat, Session, Track, track_input_channel};
use soundcraft_time::Samples;

/// Level, dim, mono, mute and talkback changes ramp over this long (seconds), so none clicks.
pub(crate) const RAMP_S: f32 = 0.010;

/// What a solo does: solo in place, or listen after (AFL) or before (PFL) the fader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Listen {
    #[default]
    Off,
    Afl,
    Pfl,
}

/// The listen mode `options.solo_mode` asks for (`afl`, `pfl`, with or without `+xor`).
pub(crate) fn listen_mode(s: &Session) -> Listen {
    let m = s.edit.solo_mode.as_str();
    if m.starts_with("afl") {
        Listen::Afl
    } else if m.starts_with("pfl") {
        Listen::Pfl
    } else {
        Listen::Off
    }
}

/// The control room's state on the audio thread. Sized in `sync`; never allocates rendering.
#[derive(Debug, Default)]
pub(crate) struct Monitor {
    /// The listen bus (the main mix's width x `max_block`).
    pub(crate) listen: Vec<Vec<f32>>,
    /// This block's listen mode (`Off` in an offline render).
    pub(crate) mode: Listen,
    /// A track is soloed in a listen mode: the monitor path plays the listen bus.
    pub(crate) active: bool,
    /// The monitor path's gain and mono blend (0 stereo, 1 mono) where the last block left them.
    gain: f32,
    mono: f32,
    /// Set once the first block has seated the ramps (nothing to ramp from before).
    settled: bool,
    /// Each hardware output's cue level (indexed like `Session::outputs`), and the talkback's.
    cue_gain: Vec<f32>,
    tb_gain: f32,
}

/// One step of a linear ramp from `cur` towards `target` (`step` per sample).
fn ramp(cur: f32, target: f32, step: f32) -> f32 {
    if (target - cur).abs() <= step {
        target
    } else if target > cur {
        cur + step
    } else {
        cur - step
    }
}

impl Monitor {
    /// Room for `s` (allocates: part of `sync`).
    pub(crate) fn reserve(&mut self, s: &Session, main_ch: usize, mb: usize) {
        if self.listen.len() != main_ch || self.listen.iter().any(|c| c.len() != mb) {
            self.listen = vec![vec![0.0; mb]; main_ch];
        }
        self.cue_gain.resize(s.outputs.len(), 1.0);
    }

    /// The start of a block: the listen mode and an empty listen bus. In a listen mode a solo
    /// mutes nothing, offline too (a bounce is the whole mix); the listen bus itself is live only.
    pub(crate) fn begin(&mut self, s: &Session, frames: usize, offline: bool) {
        self.mode = listen_mode(s);
        self.active = !offline && self.mode != Listen::Off && s.tracks.iter().any(|t| t.mixer.solo && !t.inactive);
        if self.active {
            for c in &mut self.listen {
                c.iter_mut().take(frames).for_each(|x| *x = 0.0);
            }
        }
    }
}

impl MixEngine {
    /// A soloed strip onto the listen bus (after `route_strip` has routed it): AFL its post-fader
    /// signal panned as to the main mix, PFL its pre-fader copy, unpanned.
    pub(crate) fn listen_strip(&mut self, t: &Track, strip: &Strip, pos: Samples, frames: usize) {
        let mode = self.monitor.mode;
        if !self.monitor.active || !t.mixer.solo || t.inactive {
            return;
        }
        let (src, at) = match mode {
            Listen::Afl => (&strip.buf, strip.post_at),
            Listen::Pfl if strip.pre.len() == strip.buf.len() => (&strip.pre, strip.pre_at),
            _ => return,
        };
        let src_fmt = strip_format(t, src.len());
        let apos = pos.saturating_sub(i64::try_from(at).unwrap_or(0));
        let stereo = match (mode, src.len()) {
            (Listen::Afl, _) => [pan_at(t, 0, apos), pan_at(t, 1, apos)],
            (_, 2) => [-1.0, 1.0],
            _ => [0.0, 0.0],
        };
        let surround = if mode == Listen::Afl { t.mixer.surround.as_ref() } else { None };
        let m = build_matrix(src_fmt, src.len(), self.main_fmt, self.monitor.listen.len(), &PanSpec { stereo, surround }, self.pan_law);
        apply_matrix_span(src, 0..frames, &mut self.monitor.listen, &m, 1.0);
    }

    /// Cue mixes: each cue's level (and mute) on its hardware output, and the talkback
    /// microphone summed into the cues that take it. After the hardware outputs' delay.
    pub(crate) fn cue_stage(&mut self, s: &Session, frames: usize) {
        let Some(cr) = s.control_room.as_ref().filter(|_| !self.offline) else { return };
        let step = 1.0 / (RAMP_S * self.sample_rate).max(1.0);
        let tb = &cr.talkback;
        let tb_to = if tb.on { db_to_gain(tb.level_db) } else { 0.0 };
        let tb_from = self.monitor.tb_gain;
        let tb_in = s.input_channels(&tb.input);
        for cue in &cr.cues {
            let Some(idx) = s.output_index(&cue.output).filter(|i| *i > 0) else { continue };
            let (Some(h), Some(g)) = (self.hw.get_mut(idx), self.monitor.cue_gain.get_mut(idx)) else { continue };
            let to = if cue.mute { 0.0 } else { db_to_gain(cue.level_db) };
            let from = *g;
            let talk = cue.talkback && (tb_from > 0.0 || tb_to > 0.0);
            for (k, c) in h.iter_mut().enumerate() {
                let (mut gv, mut tv) = (from, tb_from);
                let src = tb_in.filter(|_| talk).and_then(|(first, width)| self.input.get(track_input_channel(k, first, width)));
                for (i, x) in c.iter_mut().take(frames).enumerate() {
                    gv = ramp(gv, to, step);
                    tv = ramp(tv, tb_to, step);
                    let mic = src.and_then(|m| m.get(i)).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                    *x = *x * gv + mic * tv;
                }
            }
            *g = (0..frames).fold(from, |v, _| ramp(v, to, step));
        }
        self.monitor.tb_gain = (0..frames).fold(tb_from, |v, _| ramp(v, tb_to, step));
    }

    /// The monitor path, after the main mix is in `out` (and measured): the listen bus in its
    /// place while one is active, then the active set's mono, level, dim and mute, on the set's
    /// output. Without a control room and a listen, `out` is left as it is.
    pub(crate) fn monitor_stage(&mut self, s: &Session, frames: usize, out: &mut [Vec<f32>]) {
        if self.offline {
            return;
        }
        let cr = s.control_room.as_ref();
        if cr.is_none() && !self.monitor.active {
            self.monitor.settled = false;
            return;
        }
        if self.monitor.active {
            for (c, o) in out.iter_mut().enumerate() {
                let src = self.monitor.listen.get(c);
                for (i, d) in o.iter_mut().take(frames).enumerate() {
                    let v = src.and_then(|l| l.get(i)).copied().unwrap_or(0.0);
                    *d = if v.is_finite() { v } else { 0.0 };
                }
            }
        }
        let listen_db = if self.monitor.active { cr.map_or(0.0, |c| c.listen_db) } else { 0.0 };
        let to = match cr {
            Some(c) if c.mute => 0.0,
            Some(c) => db_to_gain(c.monitor_db() + listen_db),
            None => db_to_gain(listen_db),
        };
        let mono_to = if cr.is_some_and(|c| c.mono) { 1.0 } else { 0.0 };
        if !self.monitor.settled {
            (self.monitor.gain, self.monitor.mono, self.monitor.settled) = (to, mono_to, true);
        }
        let step = 1.0 / (RAMP_S * self.sample_rate).max(1.0);
        let (mut g, mut m) = (self.monitor.gain, self.monitor.mono);
        let stereo = out.len() >= 2 && self.main_fmt != ChannelFormat::Mono;
        for i in 0..frames {
            g = ramp(g, to, step);
            m = ramp(m, mono_to, step);
            if stereo && m > 0.0 {
                let l = out.first().and_then(|c| c.get(i)).copied().unwrap_or(0.0);
                let r = out.get(1).and_then(|c| c.get(i)).copied().unwrap_or(0.0);
                let mid = 0.5 * (l + r);
                for (c, v) in [(0usize, l), (1, r)] {
                    if let Some(x) = out.get_mut(c).and_then(|o| o.get_mut(i)) {
                        *x = v + (mid - v) * m;
                    }
                }
            }
            for o in out.iter_mut() {
                if let Some(x) = o.get_mut(i) {
                    *x *= g;
                }
            }
        }
        (self.monitor.gain, self.monitor.mono) = (g, m);
        // Another set's speakers: the monitor plays there, the main output falls silent.
        let set_out = cr.and_then(|c| c.set()).map(|set| set.output.as_str()).filter(|o| !o.is_empty());
        if let Some(h) = set_out.and_then(|o| s.output_index(o)).filter(|i| *i > 0).and_then(|i| self.hw.get_mut(i)).filter(|h| !h.is_empty()) {
            for (d, src) in h.iter_mut().zip(out.iter()) {
                for (x, v) in d.iter_mut().zip(src.iter()).take(frames) {
                    *x += *v;
                }
            }
            for o in out.iter_mut() {
                o.iter_mut().take(frames).for_each(|x| *x = 0.0);
            }
        }
    }
}
