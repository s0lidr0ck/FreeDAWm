//! Clipping guard rails for a remote performer (spec
//! `docs/superpowers/specs/2026-11-02-remote-audio-recording-design.md`, section 4): he is not an
//! engineer, so his screen says in plain words what to do with the knob on his interface, a clip
//! lights a lamp that stays lit until he clears it, and a 10-second level check before tracking
//! says turn up, turn down or good (a loudest peak between -12 and -6 dBFS).

/// The loudest part should peak in this range (dBFS).
pub const TARGET: (f32, f32) = (-12.0, -6.0);
/// How long the level check listens.
pub const CHECK_SECONDS: f64 = 10.0;
/// Below this nothing is coming in (dBFS).
pub const SILENCE: f32 = -50.0;

/// dBFS of a linear peak.
pub fn db(peak: f32) -> f32 {
    if peak.is_finite() && peak > 1e-6 { 20.0 * peak.log10() } else { -120.0 }
}

/// What the meter's colour says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Green,
    Amber,
    Red,
}

/// The meter's colour at `peak_db`: green up to -6 dBFS, amber up to -1, red above.
pub fn zone(peak_db: f32) -> Zone {
    if peak_db >= -1.0 {
        Zone::Red
    } else if peak_db >= TARGET.1 {
        Zone::Amber
    } else {
        Zone::Green
    }
}

/// What to tell the performer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advice {
    /// It clipped.
    Clipped,
    TooLoud,
    Good,
    TooQuiet,
    /// Nothing is coming in.
    Nothing,
}

impl Advice {
    /// The plain-language line.
    pub fn text(self) -> &'static str {
        match self {
            Advice::Clipped => "Too loud: it clipped. Turn the knob on your interface down a little",
            Advice::TooLoud => "Too loud: turn the knob on your interface down a little",
            Advice::Good => "Good level",
            Advice::TooQuiet => "Too quiet: turn the knob on your interface up",
            Advice::Nothing => "Nothing is coming in: check the cable and the input",
        }
    }

    /// Something to fix.
    pub fn warning(self) -> bool {
        !matches!(self, Advice::Good)
    }
}

/// Advice for the loudest peak of the last few seconds (dBFS), and whether it clipped. Only too
/// loud and clips are said while playing: quiet passages are normal.
pub fn advice_live(recent_peak_db: f32, clipped: bool) -> Advice {
    if clipped {
        Advice::Clipped
    } else if recent_peak_db > -3.0 {
        Advice::TooLoud
    } else if recent_peak_db < SILENCE {
        Advice::Nothing
    } else {
        Advice::Good
    }
}

/// The verdict on a level check's loudest peak.
pub fn verdict(max_db: f32, clipped: bool) -> Advice {
    if clipped {
        Advice::Clipped
    } else if max_db > TARGET.1 {
        Advice::TooLoud
    } else if max_db < SILENCE {
        Advice::Nothing
    } else if max_db < TARGET.0 {
        Advice::TooQuiet
    } else {
        Advice::Good
    }
}

/// The level check: he plays his loudest part for [`CHECK_SECONDS`]; fed the input's peaks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LevelCheck {
    elapsed: f64,
    max: f32,
    clipped: bool,
}

impl LevelCheck {
    pub fn new() -> LevelCheck {
        LevelCheck::default()
    }

    /// `dt` seconds went by with `peak` (linear) the loudest.
    pub fn feed(&mut self, peak: f32, dt: f64) {
        if dt.is_finite() && dt > 0.0 {
            self.elapsed += dt.min(1.0);
        }
        if peak.is_finite() {
            self.max = self.max.max(peak.abs());
            self.clipped |= peak.abs() >= crate::audio::CLIP_LEVEL;
        }
    }

    /// How far along (0..1).
    pub fn progress(&self) -> f64 {
        (self.elapsed / CHECK_SECONDS).clamp(0.0, 1.0)
    }

    /// The loudest peak so far (dBFS).
    pub fn max_db(&self) -> f32 {
        db(self.max)
    }

    /// The verdict, once it has listened long enough.
    pub fn result(&self) -> Option<Advice> {
        (self.elapsed >= CHECK_SECONDS).then(|| verdict(self.max_db(), self.clipped))
    }

    /// What the check found, said for the booth's screen ("Level check: good, peak -8 dB").
    pub fn summary(&self) -> Option<String> {
        let a = self.result()?;
        let word = match a {
            Advice::Clipped => "clipped",
            Advice::TooLoud => "too loud",
            Advice::Good => "good",
            Advice::TooQuiet => "too quiet",
            Advice::Nothing => "no signal",
        };
        Some(format!("Level check: {word}, peak {:.0} dB", self.max_db().max(-99.0)))
    }
}

/// The loudest of the recent peaks: `(age seconds, linear peak)` kept for `window` seconds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PeakHold {
    peaks: Vec<(f64, f32)>,
    now: f64,
}

impl PeakHold {
    /// `dt` seconds went by with `peak` the loudest; keeps `window` seconds.
    pub fn feed(&mut self, peak: f32, dt: f64, window: f64) {
        if dt.is_finite() && dt > 0.0 {
            self.now += dt.min(1.0);
        }
        if peak.is_finite() {
            self.peaks.push((self.now, peak.abs()));
        }
        let from = self.now - window;
        self.peaks.retain(|(t, _)| *t >= from);
        if self.peaks.len() > 4096 {
            self.peaks.drain(..2048);
        }
    }

    pub fn max_db(&self) -> f32 {
        db(self.peaks.iter().map(|p| p.1).fold(0.0, f32::max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meter_colours() {
        assert_eq!(zone(-20.0), Zone::Green);
        assert_eq!(zone(-6.5), Zone::Green);
        assert_eq!(zone(-4.0), Zone::Amber);
        assert_eq!(zone(-0.5), Zone::Red);
    }

    #[test]
    fn plain_language_while_playing() {
        assert_eq!(advice_live(-1.0, true), Advice::Clipped);
        assert!(advice_live(-1.0, true).text().to_lowercase().contains("turn the knob on your interface down"));
        assert_eq!(advice_live(-2.0, false), Advice::TooLoud);
        assert_eq!(advice_live(-30.0, false), Advice::Good, "quiet passages are normal while playing");
        assert_eq!(advice_live(-70.0, false), Advice::Nothing);
    }

    #[test]
    fn the_level_check_says_up_down_or_good() {
        let run = |peak: f32| {
            let mut c = LevelCheck::new();
            for i in 0..110 {
                assert!(c.result().is_none() || i >= 100);
                c.feed(if i == 50 { peak } else { peak * 0.5 }, 0.1);
            }
            c
        };
        assert_eq!(run(0.35).result(), Some(Advice::Good), "-9 dBFS");
        assert_eq!(run(0.7).result(), Some(Advice::TooLoud), "-3 dBFS");
        assert_eq!(run(1.0).result(), Some(Advice::Clipped));
        assert_eq!(run(0.1).result(), Some(Advice::TooQuiet), "-20 dBFS");
        assert_eq!(run(0.001).result(), Some(Advice::Nothing));
        assert_eq!(run(0.35).summary().as_deref(), Some("Level check: good, peak -9 dB"));
        let mut c = LevelCheck::new();
        c.feed(0.3, 5.0);
        assert!(c.result().is_none(), "a long frame counts at most a second");
        assert!((c.progress() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn the_peak_hold_forgets_old_peaks() {
        let mut h = PeakHold::default();
        h.feed(0.9, 0.1, 3.0);
        for _ in 0..40 {
            h.feed(0.1, 0.1, 3.0);
        }
        assert!((h.max_db() - -20.0).abs() < 0.01);
    }
}
