//! The spectral track view: an audio clip drawn as a spectrogram (time across, frequency up on a
//! log scale from 30 Hz to the top of the band, colour by level) in place of its waveform.
//!
//! Columns sit on absolute timeline positions (as the waveform's do), each the spectrum of a
//! 2048-sample window of the clip's file around its middle, reduced to [`BANDS`] log bands. They
//! are kept in a per-thread cache keyed by file, column width and index, so scrolling and
//! redrawing only analyse the columns newly in view, at most [`NEW_PER_FRAME`] a frame (the rest
//! fill in over the next frames).

use egui::{Color32, Mesh, Painter, Rect, pos2};
use soundcraft_model::{Clip, Session, SourceAudio, SourceId};
use std::collections::HashMap;

/// Frequency bands per column.
pub const BANDS: usize = 48;
/// Points per column.
const COLUMN_W: f32 = 4.0;
/// The FFT window.
pub const FFT: usize = 2048;
/// Lowest frequency drawn.
const LOW_HZ: f32 = 30.0;
/// Level range drawn: this many dB below full scale is black.
const RANGE_DB: f32 = 90.0;
/// Columns analysed at most per frame.
const NEW_PER_FRAME: usize = 400;
/// Columns kept in the cache before it starts over.
const MAX_CACHED: usize = 60_000;

type Key = (SourceId, usize, u64, i64);

thread_local! {
    static CACHE: std::cell::RefCell<(HashMap<Key, [u8; BANDS]>, soundcraft_dsp::spectrum::Analyzer, usize, u64)> =
        std::cell::RefCell::new((HashMap::new(), soundcraft_dsp::spectrum::Analyzer::default(), 0, 0));
}

/// The colour of a level `v` (0 black .. 255 full scale): black, violet, red, orange, yellow, white.
pub fn heat(v: u8) -> Color32 {
    const STOPS: [(f32, [f32; 3]); 6] = [
        (0.0, [0.0, 0.0, 0.0]),
        (0.25, [60.0, 10.0, 110.0]),
        (0.5, [190.0, 30.0, 70.0]),
        (0.7, [240.0, 120.0, 20.0]),
        (0.88, [250.0, 220.0, 60.0]),
        (1.0, [255.0, 255.0, 240.0]),
    ];
    let x = f32::from(v) / 255.0;
    let mut out = [0.0f32; 3];
    for w in STOPS.windows(2) {
        let (Some(&(a, ca)), Some(&(b, cb))) = (w.first(), w.get(1)) else { continue };
        if x >= a && x <= b {
            let t = if b > a { (x - a) / (b - a) } else { 0.0 };
            for (o, (p, q)) in out.iter_mut().zip(ca.iter().zip(cb.iter())) {
                *o = p + (q - p) * t;
            }
        }
    }
    Color32::from_rgb(out[0] as u8, out[1] as u8, out[2] as u8)
}

/// The band (0 = lowest) each FFT bin `k` falls in, at `rate`: log-spaced from [`LOW_HZ`] to
/// half the rate. `None` below [`LOW_HZ`].
pub fn band_of(k: usize, rate: f32) -> Option<usize> {
    let hz = soundcraft_dsp::spectrum::bin_hz(k, FFT, rate);
    let top = (rate * 0.5).max(LOW_HZ * 2.0);
    (hz >= LOW_HZ).then(|| ((hz / LOW_HZ).ln() / (top / LOW_HZ).ln() * BANDS as f32).floor().clamp(0.0, BANDS as f32 - 1.0) as usize)
}

/// One column: the levels (0..255) of each band of the window of `audio` (all channels summed)
/// centred on file frame `centre`.
pub fn column(an: &mut soundcraft_dsp::spectrum::Analyzer, audio: &SourceAudio, centre: i64, rate: f32) -> [u8; BANDS] {
    let start = centre - (FFT / 2) as i64;
    let mut mono = vec![0.0f32; FFT];
    let mut ch = vec![0.0f32; FFT];
    let channels = audio.num_channels().max(1);
    let (skip, from) = if start < 0 { (usize::try_from(-start).unwrap_or(FFT).min(FFT), 0usize) } else { (0, usize::try_from(start).unwrap_or(0)) };
    for c in 0..channels {
        ch.iter_mut().for_each(|x| *x = 0.0);
        if let Some(dst) = ch.get_mut(skip..) {
            audio.read(c, from, dst);
        }
        for (m, x) in mono.iter_mut().zip(&ch) {
            *m += x / channels as f32;
        }
    }
    let mut db = Vec::new();
    an.magnitude_db_into(&mono, FFT, &mut db);
    let mut out = [0u8; BANDS];
    let mut best = [f32::NEG_INFINITY; BANDS];
    for (k, v) in db.iter().enumerate() {
        if let Some(b) = band_of(k, rate)
            && let Some(m) = best.get_mut(b)
        {
            *m = m.max(*v);
        }
    }
    // Bands too narrow to hold a bin (the lowest ones) take the level of the band above.
    let mut last = f32::NEG_INFINITY;
    for b in (0..BANDS).rev() {
        if let Some(m) = best.get_mut(b) {
            if m.is_finite() {
                last = *m;
            } else {
                *m = last;
            }
        }
    }
    for (o, m) in out.iter_mut().zip(best) {
        *o = if m.is_finite() { ((m + RANGE_DB) / RANGE_DB * 255.0).clamp(0.0, 255.0) as u8 } else { 0 };
    }
    out
}

/// Draw `clip` (of `audio`) as a spectrogram across `body`, mapped as the Edit window's
/// timeline (`tl`). Its clip gain is not shown (the spectrum of the file as it is).
pub fn draw(painter: &Painter, s: &Session, tl: Rect, clip: &Clip, audio: &SourceAudio, frame: u64, body: Rect) {
    let Some(src) = clip.source() else { return };
    let spp = s.edit.zoom.samples_per_px.max(0.01);
    let scroll = s.edit.zoom.scroll as f64;
    let vis = painter.clip_rect().intersect(body);
    if !vis.is_positive() {
        return;
    }
    let step = spp * f64::from(COLUMN_W);
    let x_of = |pos: f64| tl.min.x + ((pos - scroll) / spp) as f32;
    let pos_at = |x: f32| scroll + f64::from(x - tl.min.x) * spp;
    let k0 = (pos_at(vis.min.x).max(clip.start as f64) / step).floor() as i64;
    let k1 = (pos_at(vis.max.x).min(clip.end() as f64) / step).ceil() as i64;
    let rate = s.sample_rate.as_f64() as f32;
    let ratio = clip.stretch_ratio();
    let key_step = step.to_bits();
    let band_h = body.height() / BANDS as f32;
    let mut mesh = Mesh::default();
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let (map, an, fresh, at_frame) = &mut *c;
        if *at_frame != frame {
            (*at_frame, *fresh) = (frame, 0);
        }
        if map.len() > MAX_CACHED {
            map.clear();
        }
        for k in k0..k1.max(k0) {
            let mid = (k as f64 + 0.5) * step;
            let key: Key = (src, clip.source_offset().max(0) as usize, key_step, k);
            let levels = match map.get(&key) {
                Some(l) => *l,
                None if *fresh < NEW_PER_FRAME => {
                    *fresh += 1;
                    // The file frame under the column's middle (through the clip's stretch and
                    // reverse).
                    let rel = (mid - clip.start as f64).max(0.0);
                    let rel = if clip.reversed { clip.length as f64 - rel } else { rel };
                    let file = clip.source_offset() as f64 + rel / ratio.max(1e-6);
                    let l = column(an, audio, file.round() as i64, rate);
                    map.insert(key, l);
                    l
                }
                None => continue,
            };
            let (x0, x1) = (x_of(k as f64 * step).max(vis.min.x), x_of((k + 1) as f64 * step).min(vis.max.x));
            if x1 <= x0 {
                continue;
            }
            for (b, v) in levels.iter().enumerate() {
                let y1 = body.max.y - band_h * b as f32;
                let r = Rect::from_min_max(pos2(x0, y1 - band_h), pos2(x1, y1));
                mesh.add_colored_rect(r, heat(*v));
            }
        }
    });
    painter.add(mesh);
}

/// Whether some columns of the clip on screen are still to be analysed (draw again next frame).
pub fn pending() -> bool {
    CACHE.with(|c| c.borrow().2 >= NEW_PER_FRAME)
}
