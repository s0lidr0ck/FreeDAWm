//! Scout guides: Claude's suggestions as read from the answer's `scout-guides` block, and the EQ
//! ones drawn on the track's Clean Q graph. Visual only: a guide never creates or moves a band.
//!
//! The drawing follows the FireStation CleanQ editor's own guides (`CleanQGraph.h`
//! `paintGuideRegions`/`paintGuideChips`, `CleanQGuides.h` `colourFor`): a tinted column from
//! `lo` to `hi` with its edges marked, a dashed line at its centre, and a numbered chip "N  label"
//! along the top, one colour per guide. Scout adds the target gain range as a band across the
//! column with a dotted line at its middle. The geometry is [`guide_rects`], so any CleanQ editor
//! can call [`overlay`] (or draw from `guide_rects` itself).

use crate::SoundApp;
use crate::theme::{Tokens, is_light};
use egui::{Color32, CornerRadius, FontId, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::Value;
use soundcraft_model::{Guide, Insert, TrackId};

/// Guides kept from one answer (the CleanQ editor's `GuideList::maxGuides`).
pub const MAX_GUIDES: usize = 24;
/// The guide frequency range, Hz, and the gain range, dB (as the package's rules tell Claude).
pub const HZ: (f32, f32) = (20.0, 20_000.0);
pub const GAIN_DB: f32 = 30.0;
/// The longest label or text field kept.
const TEXT_MAX: usize = 80;

/// One colour per guide, so a row in the panel and its region on the graph read as one thing.
/// The CleanQ editor's `GuideList::colourFor` (MCOLORS in the original).
const COLOURS: [Color32; 8] = [
    Color32::from_rgb(0xFF, 0xD1, 0x66),
    Color32::from_rgb(0xFF, 0x8A, 0x5C),
    Color32::from_rgb(0x7B, 0xE0, 0xB0),
    Color32::from_rgb(0xC9, 0xA2, 0xFF),
    Color32::from_rgb(0xFF, 0x7A, 0xB6),
    Color32::from_rgb(0x7F, 0xD8, 0xFF),
    Color32::from_rgb(0xFF, 0xB3, 0x47),
    Color32::from_rgb(0xB8, 0xFF, 0x7A),
];

/// Guide `index`'s colour (0-based, among one answer's EQ guides). In the light theme the pastel
/// is deepened so it still reads on a pale graph.
pub fn colour_for(index: usize) -> Color32 {
    let c = COLOURS[index % COLOURS.len()];
    if is_light() { Color32::from_rgb((c.r() as f32 * 0.62) as u8, (c.g() as f32 * 0.62) as u8, (c.b() as f32 * 0.62) as u8) } else { c }
}

fn clip(s: &str) -> String {
    s.trim().chars().take(TEXT_MAX).collect()
}

/// The text of the last fenced code block that names `scout-guides`, and where it starts.
fn guides_block(answer: &str) -> Option<(usize, &str)> {
    let mut found = None;
    let mut rest = 0;
    while let Some(open) = answer.get(rest..).and_then(|t| t.find("```")).map(|i| rest + i) {
        let body_start = answer.get(open..).and_then(|t| t.find('\n')).map_or(answer.len(), |i| open + i + 1);
        let close = answer.get(body_start..).and_then(|t| t.find("```")).map(|i| body_start + i);
        let body = answer.get(body_start..close.unwrap_or(answer.len())).unwrap_or("");
        if body.contains("scout-guides") {
            found = Some((open, body));
        }
        match close {
            Some(c) => rest = c + 3,
            None => break,
        }
    }
    found
}

/// The answer as shown: without its guides block (the guides are listed as rows instead). While
/// an answer streams in, an unfinished block is hidden too.
pub fn answer_body(answer: &str) -> &str {
    if let Some((open, _)) = guides_block(answer) {
        return answer.get(..open).unwrap_or(answer).trim_end();
    }
    // Streaming: a block opened and not yet closed.
    if let Some(open) = answer.rfind("```json")
        && !answer.get(open + 7..).is_some_and(|t| t.contains("```"))
    {
        return answer.get(..open).unwrap_or(answer).trim_end();
    }
    answer
}

fn num(v: &Value, key: &str) -> Option<f32> {
    let n = match v.get(key)? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    let f = n as f32;
    f.is_finite().then_some(f)
}

fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => clip(s),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// One guide from the block, or why it was dropped.
fn guide(v: &Value, known: &[String]) -> Result<Guide, String> {
    let kind = v.get("kind").and_then(Value::as_str).ok_or("not a guide")?;
    let named = text(v, "track");
    let track = known.iter().find(|k| k.eq_ignore_ascii_case(&named)).cloned().ok_or_else(|| format!("unknown track \"{named}\""))?;
    match kind {
        "eq" => {
            let (lo, hi) = (num(v, "lo_hz").ok_or("no usable lo_hz")?, num(v, "hi_hz").ok_or("no usable hi_hz")?);
            if lo >= hi {
                return Err(format!("lo_hz {lo} is not below hi_hz {hi}"));
            }
            if lo < HZ.0 || hi > HZ.1 {
                return Err(format!("{lo}–{hi} Hz is outside 20–20000 Hz"));
            }
            let (a, b) = (num(v, "gain_lo_db").ok_or("no usable gain_lo_db")?, num(v, "gain_hi_db").ok_or("no usable gain_hi_db")?);
            if a.abs() > GAIN_DB || b.abs() > GAIN_DB {
                return Err(format!("gain {a}..{b} dB is outside ±30 dB"));
            }
            // A Q hint that makes no sense is left out; the region still stands.
            let q = num(v, "q").filter(|q| (0.05..=50.0).contains(q));
            Ok(Guide::Eq {
                track,
                lo_hz: lo,
                hi_hz: hi,
                gain_lo_db: a.min(b),
                gain_hi_db: a.max(b),
                dynamic: v.get("dynamic").and_then(Value::as_bool).unwrap_or(false),
                q,
                label: text(v, "label"),
            })
        }
        "module" => {
            let module = text(v, "module");
            if module.is_empty() {
                return Err("no module named".into());
            }
            Ok(Guide::Module {
                track,
                module,
                param: text(v, "param"),
                current: text(v, "current"),
                range: text(v, "range"),
                reason: text(v, "reason"),
            })
        }
        other => Err(format!("unknown kind \"{}\"", clip(other))),
    }
}

/// The guides in Claude's answer: the last fenced block with `"format": "scout-guides"`. Guides
/// naming a track not in `known_tracks` (the package's track names), or with numbers outside the
/// package's rules (NaN, lo ≥ hi, outside 20..20000 Hz, gain outside ±30 dB), are dropped, each
/// with a note. No block at all is not an error: the answer suggested nothing to draw.
pub fn parse_guides(answer: &str, known_tracks: &[String]) -> (Vec<Guide>, Vec<String>) {
    let Some((_, body)) = guides_block(answer) else {
        return (Vec::new(), Vec::new());
    };
    let v: Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(_) => return (Vec::new(), vec!["The guides block isn't valid JSON, so no guides are shown".into()]),
    };
    let Some(items) = v.get("guides").and_then(Value::as_array) else {
        return (Vec::new(), vec!["The guides block has no list of guides".into()]);
    };
    let mut guides = Vec::new();
    let mut notes = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if guides.len() >= MAX_GUIDES {
            notes.push(format!("Only the first {MAX_GUIDES} guides are kept"));
            break;
        }
        match guide(item, known_tracks) {
            Ok(g) => guides.push(g),
            Err(why) => notes.push(format!("Guide {} dropped: {why}", i + 1)),
        }
    }
    (guides, notes)
}

/// A guide that can be drawn: finite, ordered, inside the audio band.
fn drawable(g: &Guide) -> Option<(f32, f32, f32, f32, &str)> {
    match g {
        Guide::Eq { lo_hz, hi_hz, gain_lo_db, gain_hi_db, label, .. } => {
            let ok = [*lo_hz, *hi_hz, *gain_lo_db, *gain_hi_db].iter().all(|x| x.is_finite()) && *lo_hz >= HZ.0 && *hi_hz <= HZ.1 && lo_hz < hi_hz;
            ok.then_some((*lo_hz, *hi_hz, gain_lo_db.min(*gain_hi_db), gain_lo_db.max(*gain_hi_db), label.as_str()))
        }
        Guide::Module { .. } => None,
    }
}

/// Where each EQ guide of `guides` (one answer's list) goes on a graph `rect`: its index in
/// `guides`, its lo..hi column (the full height), its colour and its chip text "N  label". N
/// numbers the answer's EQ guides from 1 in order, across every track, exactly as the panel
/// numbers them ([`eq_number`]). Guides that can't be drawn are skipped but keep their number.
pub fn guide_rects(guides: &[Guide], rect: Rect, freq_to_x: impl Fn(Rect, f32) -> f32) -> Vec<(usize, Rect, Color32, String)> {
    let mut out = Vec::new();
    for (i, g) in guides.iter().enumerate() {
        let Some(n) = eq_number(guides, i) else { continue };
        let Some((lo, hi, _, _, label)) = drawable(g) else { continue };
        let (x0, x1) = (freq_to_x(rect, lo), freq_to_x(rect, hi));
        if !(x0.is_finite() && x1.is_finite()) {
            continue;
        }
        let x0 = x0.clamp(rect.left(), rect.right());
        let x1 = x1.clamp(x0, rect.right());
        let col = Rect::from_x_y_ranges(x0..=x1.max((x0 + 2.0).min(rect.right())), rect.y_range());
        let text = if label.is_empty() { format!("{}  {}", n + 1, hz_text(lo, hi)) } else { format!("{}  {label}", n + 1) };
        out.push((i, col, colour_for(n), text));
    }
    out
}

/// Guide `i`'s number among the answer's EQ guides (0-based), or `None` for a module guide. The
/// panel's swatches and the graph's chips both use it, so they always agree.
pub fn eq_number(guides: &[Guide], i: usize) -> Option<usize> {
    matches!(guides.get(i), Some(Guide::Eq { .. })).then(|| guides.iter().take(i).filter(|g| matches!(g, Guide::Eq { .. })).count())
}

fn hz(f: f32) -> String {
    if f >= 1000.0 { format!("{}k", (f / 100.0).round() / 10.0) } else { format!("{}", f.round()) }
}

/// "2.5k–4k Hz".
pub fn hz_text(lo: f32, hi: f32) -> String {
    format!("{}–{} Hz", hz(lo), hz(hi))
}

/// The guides of the answer whose guides are shown.
pub fn shown(s: &soundcraft_model::Session) -> &[Guide] {
    s.scout.guides_shown.and_then(|id| s.scout.turns.iter().find(|t| t.id == id)).map_or(&[], |t| t.guides.as_slice())
}

/// Draws the shown answer's EQ guides for `track` on its Clean Q graph `rect`. Nothing for any
/// other module, or when no answer's guides are shown. Paint only: it takes no input.
pub fn overlay(app: &SoundApp, ui: &Ui, track: TrackId, ins: &Insert, rect: Rect) {
    if ins.plugin != "fs_cleanq" || !rect.is_finite() || rect.width() < 4.0 {
        return;
    }
    let s = app.engine.session();
    let Some(name) = s.tracks.iter().find(|t| t.id == track).map(|t| t.name.as_str()) else { return };
    let guides = shown(s);
    let mine = |i: &usize| matches!(guides.get(*i), Some(Guide::Eq { track, .. }) if track == name);
    let rects: Vec<_> = guide_rects(guides, rect, crate::module_graphs::freq_to_x).into_iter().filter(|(i, ..)| mine(i)).collect();
    if rects.is_empty() {
        return;
    }
    let p = ui.painter().with_clip_rect(rect);
    let t = Tokens::current();
    let light = is_light();
    let (fill_a, edge_a, line_a) = if light { (0.10, 0.45, 0.70) } else { (0.10, 0.22, 0.45) };
    let gain_y = |db: f32| crate::module_graphs::db_y(rect, db, crate::module_graphs::CQ_RANGE);
    // The regions, behind the chips.
    for (i, col, c, _) in &rects {
        let Some((lo, hi, g_lo, g_hi, _)) = guides.get(*i).and_then(drawable) else { continue };
        p.rect_filled(*col, CornerRadius::ZERO, c.gamma_multiply(fill_a));
        for x in [col.left(), col.right()] {
            p.vline(x, col.y_range(), Stroke::new(1.0, c.gamma_multiply(edge_a)));
        }
        // The centre, where a band would land (geometric: the graph is logarithmic).
        let xc = crate::module_graphs::freq_to_x(rect, (lo * hi).sqrt());
        let mut y = col.top();
        while y < col.bottom() {
            p.vline(xc, y..=(y + 3.0).min(col.bottom()), Stroke::new(1.0, c.gamma_multiply(line_a)));
            y += 7.0;
        }
        // The target gain range across the column, with a dotted line at its middle.
        let (y0, y1) = (gain_y(g_hi), gain_y(g_lo));
        let band = Rect::from_x_y_ranges(col.x_range(), y0.min(y1 - 3.0)..=y1.max(y0 + 3.0));
        p.rect_filled(band, CornerRadius::same(2), c.gamma_multiply(fill_a * 1.8));
        let ym = gain_y((g_lo + g_hi) * 0.5);
        let mut x = col.left() + 1.0;
        while x < col.right() {
            p.hline(x..=(x + 1.5).min(col.right()), ym, Stroke::new(1.5, c.gamma_multiply(line_a + 0.2)));
            x += 4.0;
        }
    }
    // The chips along the top, stacked where they would collide (as the CleanQ editor does).
    let font = FontId::proportional(10.0);
    let mut row_ends: Vec<f32> = Vec::new();
    for (_, col, c, label) in &rects {
        let galley = p.layout_no_wrap(label.clone(), font.clone(), *c);
        let (w, h) = (galley.size().x + 12.0, 15.0);
        let x = (col.left() + 2.0).clamp(rect.left(), (rect.right() - w).max(rect.left()));
        let row = row_ends.iter().position(|end| x >= *end).unwrap_or(row_ends.len());
        if row == row_ends.len() {
            row_ends.push(0.0);
        }
        if let Some(end) = row_ends.get_mut(row) {
            *end = x + w + 6.0;
        }
        let chip = Rect::from_min_size(pos2(x, rect.top() + 6.0 + row as f32 * (h + 3.0)), vec2(w, h));
        let bg = if light { t.panel_bg.gamma_multiply(0.94) } else { Color32::from_rgb(0x10, 0x13, 0x17).gamma_multiply(0.88) };
        p.rect_filled(chip, CornerRadius::same(3), bg);
        p.rect_stroke(chip, CornerRadius::same(3), Stroke::new(1.0, c.gamma_multiply(0.75)), StrokeKind::Inside);
        p.galley(pos2(chip.left() + 6.0, chip.center().y - galley.size().y * 0.5), galley, *c);
    }
}
