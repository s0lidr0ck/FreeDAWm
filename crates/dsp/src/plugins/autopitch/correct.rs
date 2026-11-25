//! Plumb's corrector: from each pitch the detector reports, the note to aim at (the nearest
//! enabled note of the key, changed only when the voice has clearly moved: Note hold) and how far
//! to move the voice towards it now (Retune speed, Humanize, Keep vibrato). Unsure, unvoiced or
//! breathy moments let the voice through: the correction decays to nothing in 15 ms.
//!
//! Fixed-size state only: [`Corrector::step`] never allocates.

/// Below this detector confidence the voice is let through untouched.
pub(super) const CONF_MIN: f64 = 0.75;
/// The window Keep vibrato corrects against (ms): the note's centre, not its wobble.
const CENTRE_MS: f64 = 200.0;
/// How long an unsure correction takes to fade (ms).
const RELEASE_MS: f64 = 15.0;
/// Moving-average windows hold at most this many detections.
const RING: usize = 256;

/// How the corrector behaves (worked out from the module's controls).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Settings {
    /// One-pole time of the pull towards the note (ms; 0 = at once).
    pub retune_ms: f64,
    /// Keep the singer's vibrato (correct the note's centre) instead of flattening it.
    pub keep_vibrato: bool,
    /// 0..1: notes held over half a second are corrected up to 150 ms more slowly.
    pub humanize: f64,
    /// Note hold: how much closer (cents) the voice must be to another note before the target
    /// changes.
    pub hysteresis_c: f64,
    /// The pitch averaged over this long (ms) decides the note.
    pub decide_ms: f64,
    /// A change of note must hold this long (ms) first.
    pub hold_ms: f64,
}

impl Settings {
    /// The settings from the controls: retune (ms), humanize (0..1), keep vibrato, note hold
    /// (cents). Deciding and holding scale with the retune speed (instant retune decides at once).
    pub(super) fn from_controls(retune_ms: f64, humanize: f64, keep_vibrato: bool, hold_c: f64) -> Settings {
        let retune_ms = if retune_ms.is_finite() { retune_ms.max(0.0) } else { 0.0 };
        Settings {
            retune_ms,
            keep_vibrato,
            humanize: if humanize.is_finite() { humanize.clamp(0.0, 1.0) } else { 0.0 },
            hysteresis_c: if hold_c.is_finite() { hold_c.clamp(0.0, 100.0) } else { 0.0 },
            decide_ms: (2.0 * retune_ms).min(80.0),
            hold_ms: (8.0 + 0.15 * retune_ms).clamp(8.0, 20.0),
        }
    }
}

/// The nearest note (MIDI cents, a whole semitone) to `c` whose pitch class is set in `mask`
/// (bit 0 = C), looking up to seven semitones either way. `None` when no note is enabled.
pub(super) fn nearest_in(c: f64, mask: u16) -> Option<f64> {
    if mask & 0xFFF == 0 || !c.is_finite() {
        return None;
    }
    let base = (c / 100.0).round() as i64;
    let mut best: Option<(f64, i64)> = None;
    for m in base - 7..=base + 7 {
        if mask & (1 << m.rem_euclid(12)) == 0 {
            continue;
        }
        let d = (c - m as f64 * 100.0).abs();
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, m));
        }
    }
    best.map(|b| b.1 as f64 * 100.0)
}

fn in_mask(note_cents: f64, mask: u16) -> bool {
    let m = (note_cents / 100.0).round() as i64;
    mask & (1 << m.rem_euclid(12)) != 0
}

/// A fixed ring of recent values; the mean of the last `n`.
#[derive(Clone)]
struct Window {
    buf: [f64; RING],
    len: usize,
    pos: usize,
}

impl Window {
    const fn new() -> Window {
        Window { buf: [0.0; RING], len: 0, pos: 0 }
    }
    fn clear(&mut self) {
        self.len = 0;
        self.pos = 0;
    }
    fn push(&mut self, v: f64) {
        self.buf[self.pos] = v;
        self.pos = (self.pos + 1) % RING;
        self.len = (self.len + 1).min(RING);
    }
    fn mean_last(&self, n: usize) -> f64 {
        let n = n.clamp(1, RING).min(self.len.max(1));
        let mut s = 0.0;
        for k in 1..=n {
            s += self.buf[(self.pos + RING - k) % RING];
        }
        s / n as f64
    }
}

/// The corrector's state between detections.
#[derive(Clone)]
pub(super) struct Corrector {
    dt_ms: f64,
    voiced: bool,
    dec: Window,
    cen: Window,
    /// Pushes into `cen` since the note began (its window restarts at each new note).
    cen_n: usize,
    target: Option<f64>,
    pending: Option<(f64, f64)>,
    corr: f64,
    age_ms: f64,
}

impl Corrector {
    /// A corrector stepped every `dt_ms`.
    pub(super) fn new(dt_ms: f64) -> Corrector {
        Corrector {
            dt_ms: if dt_ms.is_finite() && dt_ms > 0.0 { dt_ms } else { 2.67 },
            voiced: false,
            dec: Window::new(),
            cen: Window::new(),
            cen_n: 0,
            target: None,
            pending: None,
            corr: 0.0,
            age_ms: 0.0,
        }
    }

    pub(super) fn reset(&mut self) {
        let dt = self.dt_ms;
        *self = Corrector::new(dt);
    }

    fn new_note(&mut self, t: f64) {
        self.target = Some(t);
        self.pending = None;
        self.age_ms = 0.0;
        self.cen.clear();
        self.cen_n = 0;
    }

    /// One detection: the pitch `p` (MIDI cents, `None` unvoiced) with confidence `conf`, in a
    /// key whose enabled notes are `mask`. Returns the note aimed at (`None`: letting the voice
    /// through) and the correction to apply now (cents).
    pub(super) fn step(&mut self, s: &Settings, mask: u16, p: Option<f64>, conf: f64) -> (Option<f64>, f64) {
        let p = p.filter(|v| v.is_finite() && conf >= CONF_MIN && mask & 0xFFF != 0);
        let Some(p) = p else {
            self.voiced = false;
            self.target = None;
            self.pending = None;
            self.corr *= (-self.dt_ms / RELEASE_MS).exp();
            if self.corr.abs() < 1e-6 {
                self.corr = 0.0;
            }
            return (None, self.corr);
        };
        if !self.voiced {
            self.dec.clear();
            self.cen.clear();
            self.cen_n = 0;
            self.target = None;
            self.voiced = true;
        }
        let dec_n = ((s.decide_ms / self.dt_ms).round() as usize).max(1);
        self.dec.push(p);
        let dec = self.dec.mean_last(dec_n);
        let Some(cand) = nearest_in(dec, mask) else { return (None, self.corr) };
        match self.target {
            None => self.new_note(cand),
            // The key changed under the note: aim at the new key's note at once.
            Some(t) if !in_mask(t, mask) => self.new_note(cand),
            Some(t) if (cand - t).abs() > 1.0 => {
                let adv = (dec - t).abs() - (dec - cand).abs();
                if adv > s.hysteresis_c {
                    let since = match self.pending {
                        Some((c, ms)) if (c - cand).abs() < 1.0 => ms + self.dt_ms,
                        _ => self.dt_ms,
                    };
                    if since >= s.hold_ms {
                        self.new_note(cand);
                    } else {
                        self.pending = Some((cand, since));
                    }
                } else if (dec - t).abs() > 100.0 {
                    // A whole semitone away from the target: the singer has moved.
                    self.new_note(cand);
                } else {
                    self.pending = None;
                }
            }
            _ => self.pending = None,
        }
        let t = self.target.unwrap_or(cand);
        let cen_n = ((CENTRE_MS / self.dt_ms).round() as usize).max(1);
        self.cen.push(p);
        self.cen_n += 1;
        let cen = self.cen.mean_last(cen_n.min(self.cen_n));
        let mut d = (t - if s.keep_vibrato { cen } else { p }).clamp(-300.0, 300.0);
        // While a switch to another note is pending (the voice has left this note), do not pull
        // the voice back towards it: let the singer through.
        if self.pending.is_some() {
            d = 0.0;
        }
        let ramp = ((self.age_ms - 150.0) / 350.0).clamp(0.0, 1.0);
        let tau = s.retune_ms + s.humanize * 150.0 * ramp;
        let a = if tau <= 0.0 { 1.0 } else { 1.0 - (-self.dt_ms / tau).exp() };
        self.corr += (d - self.corr) * a;
        if !self.corr.is_finite() {
            self.corr = 0.0;
        }
        self.age_ms += self.dt_ms;
        (Some(t), self.corr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_enabled_note_skips_switched_off_ones() {
        let c_major = 0b1010_1011_0101;
        assert_eq!(nearest_in(6040.0, c_major), Some(6000.0), "C sung sharp stays C");
        assert_eq!(nearest_in(6110.0, c_major), Some(6200.0), "nearer D than C");
        assert_eq!(nearest_in(6160.0, c_major), Some(6200.0));
        // E switched off: E goes to the nearer of D and F (F, a semitone away).
        assert_eq!(nearest_in(6400.0, c_major & !(1 << 4)), Some(6500.0));
        assert_eq!(nearest_in(6400.0, 0), None);
    }
}
