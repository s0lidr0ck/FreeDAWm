//! Clip waveforms, drawn per physical pixel, or as a line through the samples at the sample level.

use egui::{Color32, Mesh, Painter, Rect, Shape, Stroke, pos2, vec2};
use soundcraft_model::{Clip, Session, SourceAudio};
use soundcraft_time::Samples;

/// Clean Vocal's cleaned audio as the waveform blends it in: its share, where it starts in the
/// original's time, and its audio.
type Alt<'a> = (f32, Samples, &'a SourceAudio);

/// Draw a clip's audio across `body`, one band per channel, `channels` of them at most. A clip
/// cleaned by Clean Vocal shows its current mix: the original's and the cleaned audio's peaks (or
/// samples) blended by the clip's amount, from the peak data already in memory.
pub fn draw(painter: &Painter, s: &Session, tl: Rect, clip: &Clip, audio: &SourceAudio, channels: usize, body: Rect, wave: Color32) {
    draw_mapped(painter, s, (tl.min.x, s.edit.zoom.scroll as f64, s.edit.zoom.samples_per_px), clip, audio, channels, body, wave);
}

/// [`draw`] with its own time mapping `(x of time `scroll`, scroll, samples per point)` instead of
/// the Edit window's timeline (the Vocal Editor's roll is narrower than the timeline).
#[allow(clippy::too_many_arguments)]
pub fn draw_mapped(
    painter: &Painter,
    s: &Session,
    map: (f32, f64, f64),
    clip: &Clip,
    audio: &SourceAudio,
    channels: usize,
    body: Rect,
    wave: Color32,
) {
    // A stretched clip is drawn as it plays: its file samples spread over the clip's length. It
    // is drawn as an unstretched clip in a space `ratio` times finer (the same start, its
    // lengths in file samples), so the waveform's file reads stay one sample per position.
    let ratio = clip.stretch_ratio();
    if (ratio - 1.0).abs() > 1e-9 && clip.is_audio() {
        let (tl_x, scroll, spp) = map;
        let start = clip.start as f64;
        let fine = (tl_x, start - (start - scroll) / ratio, spp / ratio);
        let span = |n: Samples| clip.source_span(n);
        let mut c =
            Clip { length: span(clip.length), stretch: 1.0, loop_len: span(clip.loop_len), loop_phase: span(clip.loop_phase), ..clip.clone() };
        c.fade_in.len = span(c.fade_in.len);
        c.fade_out.len = span(c.fade_out.len);
        c.gain_env.iter_mut().for_each(|p| p.0 = span(p.0));
        draw_mapped(painter, s, fine, &c, audio, channels, body, wave);
        return;
    }
    // A looped clip (Loop in Place) is drawn one repeat at a time, each the loop's window as a
    // clip of its own (reversed too when the clip is), with the whole clip's gain and fades.
    if clip.loop_len > 0 && clip.is_audio() {
        let (tl_x, scroll, spp) = map;
        let x_of = |pos: Samples| tl_x + ((pos as f64 - scroll) / spp.max(0.01)) as f32;
        let r = painter.clip_rect();
        let first = clip.start.saturating_sub(clip.loop_phase.rem_euclid(clip.loop_len));
        let mut at = first;
        // Only the repeats on screen, and never more than a screenful of them.
        let mut n = 0;
        while at < clip.end() && n < 4096 {
            let end = at.saturating_add(clip.loop_len);
            let (x0, x1) = (x_of(at.max(clip.start)), x_of(end.min(clip.end())));
            if x1 >= r.min.x && x0 <= r.max.x {
                let seg = Clip { start: at, length: clip.loop_len, loop_len: 0, loop_phase: 0, ..clip.clone() };
                let p = painter.with_clip_rect(r.intersect(Rect::from_x_y_ranges(x0..=x1, r.y_range())));
                draw_one(&p, s, map, &seg, clip, audio, channels, body, wave);
            }
            if x0 > r.max.x {
                break;
            }
            at = end;
            n += 1;
        }
        return;
    }
    draw_one(painter, s, map, clip, clip, audio, channels, body, wave);
}

/// One clip's waveform: `clip` places the material, `gains` (the clip itself, or the looped clip
/// a repeat belongs to) its gain and fades on the timeline.
#[allow(clippy::too_many_arguments)]
fn draw_one(
    painter: &Painter,
    s: &Session,
    map: (f32, f64, f64),
    clip: &Clip,
    gains: &Clip,
    audio: &SourceAudio,
    channels: usize,
    body: Rect,
    wave: Color32,
) {
    let alt: Option<Alt<'_>> = clip.clean_alt().filter(|k| k.wet() > 0.0).and_then(|k| s.pool.get(k.source).map(|a| (k.wet(), k.origin, &**a)));
    // A clip reversed in place (not looped) is drawn as it plays: its window mirrored end for
    // end. The waveform is worked out for the mirror image of what is visible, then flipped
    // about the clip's middle.
    let (tl_x, scroll, spp) = map;
    let x_of = |pos: Samples| tl_x + ((pos as f64 - scroll) / spp.max(0.01)) as f32;
    let flip = (clip.reversed && clip.loop_len <= 0 && clip.is_audio()).then(|| (x_of(clip.start) + x_of(clip.end())) * 0.5);
    let shown = match flip {
        Some(axis) => {
            let r = painter.clip_rect();
            painter.with_clip_rect(Rect::from_x_y_ranges(2.0 * axis - r.max.x..=2.0 * axis - r.min.x, r.y_range()))
        }
        None => painter.clone(),
    };
    for (ch, band) in bands(body, channels.min(audio.peaks.len()).max(1)).enumerate() {
        channel(&shown, painter, s, map, (clip, gains), audio, alt, ch, band, wave, flip);
    }
}

/// The Pencil at sample level: the channel band of `body` (one band per channel, `channels` of
/// them) that `y` is in, and the file sample value a point drawn there stands for at timeline
/// `pos` (the inverse of the sample-level line: the clip's gain and the waveform zoom taken off).
pub fn pencil_value(s: &Session, clip: &Clip, channels: usize, body: Rect, pos: Samples, y: f32) -> (usize, f32) {
    let n = channels.max(1);
    let h = body.height() / n as f32;
    let ch = if h > 0.0 { (((y - body.min.y) / h).floor().max(0.0) as usize).min(n - 1) } else { 0 };
    let band = bands(body, n).nth(ch).unwrap_or(body);
    let half = band.height() * 0.46;
    let shown = if half > 0.0 { ((band.center().y - y) / half).clamp(-1.0, 1.0) } else { 0.0 };
    let gain = clip.gain_at(pos.saturating_sub(clip.start).clamp(0, clip.length)) * s.edit.zoom.waveform_zoom;
    let v = if gain.abs() > 1e-6 { shown / gain } else { shown };
    (ch, if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 })
}

/// Samples per point up to which the Pencil redraws an audio clip's samples.
pub const PENCIL_SAMPLES_PER_POINT: f64 = 1.0;

/// `(1 - w) * a + w * b` over two optional spans; the original alone where the cleaned audio has none.
fn blend_span(a: Option<(f32, f32)>, b: Option<(f32, f32)>, w: f32) -> Option<(f32, f32)> {
    match (a, b) {
        (Some((al, ah)), Some((bl, bh))) => Some(((1.0 - w) * al + w * bl, (1.0 - w) * ah + w * bh)),
        (a, None) => a,
        (None, Some((bl, bh))) => Some((w * bl, w * bh)),
    }
}

fn bands(body: Rect, n: usize) -> impl Iterator<Item = Rect> {
    let h = body.height() / n as f32;
    (0..n).map(move |k| Rect::from_min_size(pos2(body.min.x, body.min.y + h * k as f32), vec2(body.width(), h)))
}

#[allow(clippy::too_many_arguments)]
fn channel(
    painter: &Painter,
    out: &Painter,
    s: &Session,
    map: (f32, f64, f64),
    (clip, gains): (&Clip, &Clip),
    audio: &SourceAudio,
    alt: Option<Alt<'_>>,
    ch: usize,
    band: Rect,
    wave: Color32,
    flip: Option<f32>,
) {
    let Some(peaks) = audio.peaks.get(ch) else { return };
    // The cleaned audio's channel (wrapping as playback does), its share and its offset.
    let alt = alt.and_then(|(w, origin, a)| {
        let n = a.peaks.len().min(a.num_channels());
        (n > 0).then(|| (w, origin as f64, a, ch % n, a.peaks.get(ch % n))).and_then(|(w, o, a, c, p)| Some((w, o, a, c, p?)))
    });
    let Some(mut g) = Grid::at(painter, map, band) else { return };
    g.flip = flip;
    let (rectified, power, outlines) = (s.edit.flag("waveform.rectified"), s.edit.flag("waveform.power"), s.edit.flag("waveform.outlines"));
    let origin = clip.start.saturating_sub(clip.source_offset()) as f64;
    // Clip gain and fades stay where they are on the timeline: a flipped column's gain is the
    // gain where it is drawn.
    let mirror = (clip.start + clip.end()) as f64;
    let gain = |pos: f64| {
        let at = if flip.is_some() { mirror - pos } else { pos };
        gains.gain_at((at as Samples).saturating_sub(gains.start).min(gains.length).max(0)) * s.edit.zoom.waveform_zoom
    };
    // The samples shown (positions `first..last` of the source), read around what is visible: a
    // file played from disk is never read whole for its waveform.
    let first = (g.pos_at(g.vis.min.x) - origin).floor().max(0.0) as usize;
    let last = (g.pos_at(g.vis.max.x) - origin).ceil().max(0.0) as usize;
    if g.step <= 1.0 {
        let from = first.saturating_sub(1);
        let to = audio.frames().min(last.saturating_add(2));
        let (samples, from) = window(audio, ch, from, to);
        let cleaned = alt.map(|(w, o, a, c, _)| {
            let at = (from as f64 - o).max(0.0) as usize;
            (w, o, window(a, c, at, at.saturating_add(samples.len())))
        });
        let points = samples.iter().zip(from..).map(|(&v, i)| {
            let pos = origin + i as f64;
            let v = match cleaned
                .as_ref()
                .and_then(|(w, o, (c, c0))| usize::try_from(i as i64 - *o as i64).ok().and_then(|k| c.get(k.checked_sub(*c0)?)).map(|cv| (*w, *cv)))
            {
                Some((w, cv)) => (1.0 - w) * v + w * cv,
                None => v,
            };
            pos2(g.fx(g.x_at(pos)), g.cy - level(v * gain(pos)) * g.half)
        });
        out.add(Shape::line(points.collect(), Stroke::new(g.weight, wave)));
        return;
    }
    let (start, overlap) = (g.start() - origin, g.overlap());
    // Zoomed far out the overview alone draws it (the samples are only read at column edges, and
    // those few don't show then).
    let span = |a, c, from| samples_for(a, c, from, last.saturating_sub(first), g.step);
    // An overview whose fine levels are read from disk draws its coarser levels where a page
    // isn't loaded yet, and is drawn again once it is.
    let spans_of = |peaks: &soundcraft_audio_io::peaks::Peaks, samples: &[f32], at: usize, start: f64| -> Vec<Option<(f32, f32)>> {
        let (spans, complete) = if power {
            let (r, done) = peaks.rms_spans_in_checked(samples, at, start, g.step, overlap, g.cols);
            (r.into_iter().map(|r| r.map(|r| (-r, r))).collect(), done)
        } else {
            peaks.spans_in_checked(samples, at, start, g.step, overlap, g.cols)
        };
        if !complete {
            painter.ctx().request_repaint_after(REDRAW_WHILE_LOADING);
        }
        spans
    };
    let (samples, at) = span(audio, ch, start);
    let mut spans = spans_of(peaks, &samples, at, start);
    if let Some((w, o, a, c, p)) = alt {
        // Columns before the cleaned audio's start have no cleaned span (negative positions).
        let (cs, cat) = span(a, c, start - o);
        let cleaned = spans_of(p, &cs, cat, start - o);
        let first = ((o - start) / g.step).ceil().max(0.0) as usize;
        for (i, (sp, cs)) in spans.iter_mut().zip(cleaned).enumerate() {
            *sp = blend_span(*sp, if i < first { None } else { cs }, w);
        }
    }
    let base = g.cy + g.half;
    let ys: Vec<Option<(f32, f32)>> = spans
        .into_iter()
        .enumerate()
        .map(|(c, span)| {
            let (lo, hi) = span?;
            let k = gain((g.k0 + c as f64 + 0.5) * g.step);
            Some(if rectified {
                (base - level(lo.abs().max(hi.abs()) * k) * g.half * 2.0, base)
            } else {
                (g.cy - level(hi * k) * g.half, g.cy - level(lo * k) * g.half)
            })
        })
        .collect();
    g.paint(out, &ys, wave, outlines, rectified);
}

/// Samples per column up to which a waveform reads the samples at its columns' edges.
const FINE_STEP: f64 = 64.0;
/// How soon a waveform drawn while its overview's pages load is drawn again.
pub const REDRAW_WHILE_LOADING: std::time::Duration = std::time::Duration::from_millis(16);
/// Margin read around the visible samples (a peak block each side and more).
const BLOCK_MARGIN: usize = 256;

/// The samples a waveform's spans read for channel `c` of `a` from position `from` on, `len`
/// positions long, and where they start: all of them when in memory, a window around them from
/// disk, none when zoomed out beyond [`FINE_STEP`].
fn samples_for(a: &SourceAudio, c: usize, from: f64, len: usize, step: f64) -> (std::borrow::Cow<'_, [f32]>, usize) {
    let from = from.max(0.0) as usize;
    match a.channel(c) {
        Some(all) => (std::borrow::Cow::Borrowed(all), 0),
        None if step > FINE_STEP => (std::borrow::Cow::Borrowed(&[]), 0),
        None => {
            let (v, at) = window(a, c, from.saturating_sub(BLOCK_MARGIN), from.saturating_add(len).saturating_add(BLOCK_MARGIN * 2));
            (std::borrow::Cow::Owned(v), at)
        }
    }
}

/// Channel `ch` of `audio` over positions `[from, to)`, and `from` (memory is copied; a file played
/// from disk is read through its cache).
fn window(audio: &SourceAudio, ch: usize, from: usize, to: usize) -> (Vec<f32>, usize) {
    let to = to.min(audio.frames());
    let mut v = vec![0.0; to.saturating_sub(from)];
    audio.read(ch, from, &mut v);
    (v, from)
}

/// Draw a take while it records across `body`, one band per channel: `blocks` holds each channel's
/// (min, max) per block of input frames, the blocks `block_len` timeline samples long from
/// timeline position `origin`.
pub fn draw_live(painter: &Painter, s: &Session, tl: Rect, origin: f64, block_len: f64, blocks: &[Vec<(f32, f32)>], body: Rect, wave: Color32) {
    let zoom = s.edit.zoom.waveform_zoom;
    for (blocks, band) in blocks.iter().zip(bands(body, blocks.len())) {
        let Some(g) = Grid::new(painter, s, tl, band) else { continue };
        let ys: Vec<Option<(f32, f32)>> = (0..g.cols)
            .map(|c| {
                let a = g.start() + c as f64 * g.step - origin;
                let (i, j) = ((a / block_len).floor().max(0.0) as usize, ((a + g.step + g.overlap()) / block_len).ceil().max(0.0) as usize);
                let (lo, hi) =
                    blocks.get(i..j.min(blocks.len()))?.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), b| (lo.min(b.0), hi.max(b.1)));
                (lo <= hi).then(|| (g.cy - level(hi * zoom) * g.half, g.cy - level(lo * zoom) * g.half))
            })
            .collect();
        g.paint(painter, &ys, wave, false, false);
    }
}

fn level(v: f32) -> f32 {
    if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }
}

/// One column per physical pixel across the visible part of a channel's band, each summing up a
/// one-point window of timeline samples anchored to absolute positions, so scrolling doesn't
/// change what a column shows.
struct Grid {
    vis: Rect,
    tl_x: f32,
    scroll: f64,
    /// Timeline samples per point, and per column.
    spp: f64,
    step: f64,
    px: f32,
    /// A one-point line, in whole physical pixels.
    weight: f32,
    cy: f32,
    half: f32,
    /// The first column's physical pixel and absolute window index, and the column count.
    p0: f32,
    k0: f64,
    cols: usize,
    /// Drawn mirrored about this x (a reversed clip).
    flip: Option<f32>,
}

impl Grid {
    fn new(painter: &Painter, s: &Session, tl: Rect, band: Rect) -> Option<Grid> {
        Grid::at(painter, (tl.min.x, s.edit.zoom.scroll as f64, s.edit.zoom.samples_per_px), band)
    }

    /// The grid for time mapping `(x of time `scroll`, scroll, samples per point)`.
    fn at(painter: &Painter, (tl_x, scroll, spp): (f32, f64, f64), band: Rect) -> Option<Grid> {
        let vis = painter.clip_rect().intersect(band);
        if !vis.is_positive() {
            return None;
        }
        let ppp = painter.pixels_per_point();
        let px = 1.0 / ppp;
        let weight = ppp.round().max(1.0) * px;
        let spp = spp.max(0.01);
        let step = spp * f64::from(px);
        let p0 = (vis.min.x * ppp).floor();
        let k0 = ((scroll + f64::from((p0 + 0.5) * px - tl_x) * spp) / step).floor();
        let cy = ((band.center().y - weight * 0.5) * ppp).round() * px + weight * 0.5;
        let cols = ((vis.max.x * ppp).ceil() - p0).max(0.0) as usize;
        Some(Grid { vis, tl_x, scroll, spp, step, px, weight, cy, half: band.height() * 0.46, p0, k0, cols, flip: None })
    }

    fn pos_at(&self, x: f32) -> f64 {
        self.scroll + f64::from(x - self.tl_x) * self.spp
    }

    /// Where an x is drawn: itself, or mirrored for a reversed clip.
    fn fx(&self, x: f32) -> f32 {
        self.flip.map_or(x, |axis| 2.0 * axis - x)
    }

    fn x_at(&self, pos: f64) -> f32 {
        self.tl_x + ((pos - self.scroll) / self.spp) as f32
    }

    /// How far each column's window reaches into its neighbours': a column is `step` apart but sums
    /// up a whole point.
    fn overlap(&self) -> f64 {
        (self.spp - self.step).max(0.0)
    }

    /// The timeline position where the first column's window starts.
    fn start(&self) -> f64 {
        self.k0 * self.step - self.overlap() * 0.5
    }

    /// Fill the columns' (top, bottom) spans; with `outlines`, edge them in `wave` around a lighter
    /// fill, along the top only when `rectified`.
    fn paint(&self, painter: &Painter, ys: &[Option<(f32, f32)>], wave: Color32, outlines: bool, rectified: bool) {
        let mut mesh = Mesh::default();
        self.strip(&mut mesh, ys, if outlines { wave.gamma_multiply(0.55) } else { wave });
        let edges: &[fn((f32, f32)) -> f32] = match (outlines, rectified) {
            (false, _) => &[],
            (true, true) => &[|y| y.0],
            (true, false) => &[|y| y.0, |y| y.1],
        };
        for edge in edges {
            let line: Vec<Option<(f32, f32)>> = ys
                .iter()
                .enumerate()
                .map(|(c, y)| {
                    let y = edge((*y)?);
                    let near = |i: Option<usize>| i.and_then(|i| ys.get(i).copied().flatten()).map_or(y, |n| (edge(n) + y) * 0.5);
                    let (a, b) = (near(c.checked_sub(1)), near(Some(c + 1)));
                    Some((y.min(a).min(b) - self.weight * 0.5, y.max(a).max(b) + self.weight * 0.5))
                })
                .collect();
            self.strip(&mut mesh, &line, wave);
        }
        painter.add(Shape::mesh(mesh));
    }

    /// Fill each column's (top, bottom) span, at least `weight` tall, joined to its neighbours in
    /// one triangle strip and anti-aliased at both ends. Runs of equal spans share vertices.
    fn strip(&self, mesh: &mut Mesh, spans: &[Option<(f32, f32)>], color: Color32) {
        let half = self.px * 0.5;
        let thick = |(top, bot): (f32, f32)| {
            if bot - top < self.weight { ((top + bot - self.weight) * 0.5, (top + bot + self.weight) * 0.5) } else { (top, bot) }
        };
        let push = |mesh: &mut Mesh, prev: &mut Option<u32>, x: f32, (top, bot): (f32, f32)| {
            let i = mesh.vertices.len() as u32;
            let x = self.fx(x);
            mesh.colored_vertex(pos2(x, top - half), Color32::TRANSPARENT);
            mesh.colored_vertex(pos2(x, top + half), color);
            mesh.colored_vertex(pos2(x, bot - half), color);
            mesh.colored_vertex(pos2(x, bot + half), Color32::TRANSPARENT);
            if let Some(j) = *prev {
                for k in 0..3 {
                    mesh.add_triangle(j + k, j + k + 1, i + k);
                    mesh.add_triangle(j + k + 1, i + k + 1, i + k);
                }
            }
            *prev = Some(i);
        };
        let at = |c: Option<usize>| c.and_then(|c| spans.get(c).copied().flatten()).map(thick);
        let mut prev = None;
        for c in 0..spans.len() {
            let Some(span) = at(Some(c)) else {
                prev = None;
                continue;
            };
            let x = (self.p0 + c as f32 + 0.5) * self.px;
            let (before, after) = (at(c.checked_sub(1)), at(Some(c + 1)));
            if before.is_none() {
                push(mesh, &mut prev, x - half, span);
            }
            if before != Some(span) || after != Some(span) {
                push(mesh, &mut prev, x, span);
            }
            if after.is_none() {
                push(mesh, &mut prev, x + half, span);
            }
        }
    }
}
