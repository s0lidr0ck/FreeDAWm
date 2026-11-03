//! A track's timed lyrics (AI › Transcribe Lyrics…), drawn as a strip along the bottom of its lane
//! in the Edit window: each word at its time, skipped where it would overlap the previous one.

use crate::edit_window::x_of;
use crate::theme::{Tokens, regular};
use egui::{Align2, Color32, Rect, pos2};
use soundcraft_model::{Session, Track};

/// The strip's height.
pub const STRIP_H: f32 = 14.0;

/// The strip of `track`'s lyrics at the bottom of `lane` (nothing for a track without lyrics or a
/// lane too short to hold it above its clips).
pub fn draw(painter: &egui::Painter, s: &Session, track: &Track, tl: Rect, lane: Rect) {
    if !soundcraft_model::track_has_lyrics(track) || lane.height() < STRIP_H * 2.5 {
        return;
    }
    let lines = soundcraft_model::track_lines(track);
    let t = Tokens::current();
    let strip = Rect::from_min_max(pos2(lane.min.x, lane.max.y - STRIP_H), lane.max);
    painter.rect_filled(strip, 0.0, Color32::from_black_alpha(110));
    let font = regular(10.0);
    let mut free_from = f32::NEG_INFINITY;
    for line in &lines {
        let (a, b) = (x_of(s, tl, line.start), x_of(s, tl, line.end));
        if b < lane.min.x || a > lane.max.x {
            continue;
        }
        // A tick where each line starts.
        painter.vline(a, strip.y_range(), egui::Stroke::new(1.0, t.accent));
        for w in &line.words {
            let x = x_of(s, tl, w.start).max(a + 2.0);
            if x < free_from || x > lane.max.x {
                continue;
            }
            let r = painter.text(pos2(x + 1.0, strip.center().y), Align2::LEFT_CENTER, &w.text, font.clone(), Color32::from_gray(235));
            free_from = r.max.x + 4.0;
        }
    }
}
