//! The Level lane under the roll: the clip's frame level in faint grey, each note as a bar at its
//! level after the levelling gain (a faint ghost at the original level when moved), and a dashed
//! line at the typical level the levelling aims at. Clicking a bar selects its note; the sliders
//! do the levelling.

use super::layout::{Frames, RollView};
use super::{editor_track, roll, store};
use crate::SoundApp;
use crate::theme::{Tokens, regular};
use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2};
use soundcraft_engine::vocal::NoteRef;

/// The lane's dB range either side of the typical level.
const RANGE_DB: f32 = 15.0;

/// Screen y of `db` in `lane`, whose range is `typical ± 15 dB` (clamped to the lane).
pub fn lane_y(lane: Rect, typical: f32, db: f32) -> f32 {
    let lo = typical - RANGE_DB;
    let k = ((db - lo) / (2.0 * RANGE_DB)).clamp(0.0, 1.0);
    let k = if k.is_finite() { k } else { 0.5 };
    lane.bottom() - 4.0 - k * (lane.height() - 8.0)
}

/// "typical −21 dB" (whole dB, U+2212).
pub fn typical_label(db: f32) -> String {
    let r = if db.is_finite() { db.round() } else { 0.0 };
    if r < 0.0 { format!("typical \u{2212}{:.0} dB", -r) } else { format!("typical {r:.0} dB") }
}

/// Draws the lane in `rect` (its x mapping is `view`'s) and selects a note when its bar is clicked.
pub fn level_lane(app: &mut SoundApp, ui: &mut Ui, rect: Rect, view: &RollView) {
    let t = Tokens::current();
    let Some(track) = editor_track(app) else { return };
    let lane = Rect::from_min_max(pos2(view.rect.left(), rect.top()), pos2(view.rect.right(), rect.bottom()));
    let p = ui.painter().with_clip_rect(rect);
    let dark = u32::from(t.panel_bg.r()) + u32::from(t.panel_bg.g()) + u32::from(t.panel_bg.b()) < 384;
    p.rect_filled(rect, 0.0, if dark { Color32::from_rgb(32, 33, 36) } else { t.panel_bg2 });
    p.text(pos2(rect.left() + 6.0, rect.top() + 10.0), Align2::LEFT_CENTER, "Level", regular(11.0), t.text_dim);
    store(ui.ctx(), "lane", lane);
    let clips = roll::clip_views(app, track, Some((view.t0, view.t1)));
    // The typical level, as the Tuning panel's caption: the selection's, else the track's.
    let typical = roll::selection_typical(app, track);
    let lp = p.with_clip_rect(lane);
    let grey = if dark { Color32::from_rgb(92, 96, 112) } else { Color32::from_rgb(150, 154, 166) };
    let (bar_c, sel_c) =
        if dark { (Color32::from_rgb(143, 180, 216), Color32::WHITE) } else { (Color32::from_rgb(70, 110, 160), Color32::from_rgb(20, 20, 20)) };
    let mut bars = Vec::new();
    let mut ghosts = Vec::new();
    for cv in clips.iter().filter(|c| !c.stretched) {
        let fm = Frames::of(&cv.clip, cv.a.track.hop_samples);
        // The frame level, faint.
        if let Some((lo, hi)) = fm.visible(view) {
            let hi = hi.min(u32::try_from(cv.a.track.frames.len()).unwrap_or(u32::MAX));
            let step = ((hi.saturating_sub(lo)) / 2000).max(1);
            let pts: Vec<Pos2> = (lo..hi)
                .step_by(step as usize)
                .filter_map(|f| {
                    let at = fm.time(f);
                    (at >= cv.clip.start && at <= cv.clip.end())
                        .then(|| cv.a.track.frames.get(f as usize))
                        .flatten()
                        .map(|fr| pos2(view.x(at), lane_y(lane, typical, fr.level_db.max(typical - RANGE_DB))))
                })
                .collect();
            if pts.len() > 1 {
                lp.add(Shape::line(pts, Stroke::new(1.0, grey)));
            }
        }
        let Some((lo, hi)) = fm.visible(view) else { continue };
        let range = super::layout::notes_in(&cv.a, lo, hi);
        for n in cv.a.notes.get(range).unwrap_or(&[]) {
            let (t0, t1) = (fm.clamp(fm.time(n.start)), fm.clamp(fm.time(n.end)));
            if t1 <= t0 || t1 <= view.t0 || t0 >= view.t1 {
                continue;
            }
            let r = NoteRef { clip: cv.clip.id, note: n.id };
            let mid = n.start.saturating_add(n.end.saturating_sub(n.start) / 2);
            let gain = cv.curves.as_ref().and_then(|c| c.gain_db.get(mid as usize)).copied().filter(|g| g.is_finite()).unwrap_or(0.0);
            let (x0, x1) = (view.x(t0), view.x(t1).max(view.x(t0) + 2.0));
            if gain.abs() > 0.3 {
                let y = lane_y(lane, typical, n.level_db);
                let g = Rect::from_min_max(pos2(x0, y - 1.0), pos2(x1, y + 1.0));
                lp.extend(Shape::dashed_line(&[g.left_center(), g.right_center()], Stroke::new(1.0, grey.gamma_multiply(1.4)), 2.0, 2.0));
                ghosts.push((r, g));
            }
            let y = lane_y(lane, typical, n.level_db + gain);
            let b = Rect::from_min_max(pos2(x0, y - 1.5), pos2(x1, y + 1.5));
            lp.rect_filled(b, 1.5, if app.vocal.selected.contains(&r) { sel_c } else { bar_c });
            bars.push((r, b));
        }
    }
    // The typical line.
    let ty = lane_y(lane, typical, typical);
    let gold = Color32::from_rgb(201, 168, 90);
    lp.extend(Shape::dashed_line(&[pos2(lane.left(), ty), pos2(lane.right(), ty)], Stroke::new(1.0, gold), 4.0, 3.0));
    let label = typical_label(typical);
    lp.text(pos2(lane.right() - 4.0, ty - 2.0), Align2::RIGHT_BOTTOM, &label, regular(10.5), gold);
    store(ui.ctx(), "typical", label);
    let ph = app.position();
    if ph >= view.t0 && ph <= view.t1 {
        let x = view.x(ph);
        lp.line_segment([pos2(x, lane.top()), pos2(x, lane.bottom())], Stroke::new(1.5, t.playhead));
    }
    store(ui.ctx(), "level-bars", bars.clone());
    store(ui.ctx(), "level-ghosts", ghosts);
    // Clicking a bar selects its note (same modifiers as the roll).
    let resp = ui.interact(lane, ui.id().with(("vocal-level", track.0)), Sense::click());
    if resp.clicked()
        && let Some(pt) = resp.interact_pointer_pos()
    {
        let toggle = ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);
        let hit = bars.iter().rev().find(|(_, b)| b.expand2(egui::vec2(1.0, 4.0)).contains(pt)).map(|(r, _)| *r);
        roll::click_select(app, track, hit, toggle);
        if let Some(r) = hit {
            super::preview::request(app, r);
        }
    }
}
