//! The Teleprompter (Window › Teleprompter): a big, high-contrast lyric view for the singer. It
//! follows the playhead through the lines placed on the Lyrics ruler: the current line large and
//! bright with a bar showing how far through it the playhead is, the next line below it, the
//! section above. It docks as a tab, or lives in its own native window (Window › Teleprompter in
//! Separate Window), sized and placed as last left; closing that window docks it back.

use crate::SoundApp;
use crate::theme::{bold, regular};
use egui::{Align2, Color32, Rect, Ui, ViewportId, pos2, vec2};
use soundcraft_model::{LyricPlace, Session};
use soundcraft_time::Samples;

/// The separate window's viewport.
pub fn viewport() -> ViewportId {
    ViewportId::from_hash_of("soundcraft.teleprompter")
}

/// What the Teleprompter shows at a playhead position.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    /// The section heading above the line ("Chorus").
    pub section: Option<String>,
    /// The current line: "<n>", text, and how far through it the playhead is (0..1). Between
    /// lines, the next one is current with progress 0 ("get ready").
    pub current: Option<(u32, String, f32)>,
    /// Whether the playhead is inside the current line (not waiting for it).
    pub singing: bool,
    /// The line after it.
    pub next: Option<(u32, String)>,
}

fn line(s: &Session, p: &LyricPlace) -> Option<(u32, String)> {
    let sheet = &s.lyric_sheet;
    Some((sheet.number_of(p.line)?, sheet.lines.iter().find(|l| l.id == p.line)?.text.trim().to_string()))
}

/// The prompt at `at`.
pub fn prompt(s: &Session, at: Samples) -> Prompt {
    let placed = &s.lyric_sheet.placed;
    let now = placed.iter().position(|p| p.start <= at && at < p.end);
    let (cur, singing) = match now {
        Some(i) => (Some(i), true),
        None => (placed.iter().position(|p| p.start > at), false),
    };
    let current = cur.and_then(|i| placed.get(i)).and_then(|p| {
        let (n, text) = line(s, p)?;
        let k = if singing { ((at - p.start) as f64 / (p.end - p.start).max(1) as f64) as f32 } else { 0.0 };
        Some((n, text, k.clamp(0.0, 1.0)))
    });
    let next = cur.and_then(|i| placed.get(i + 1)).and_then(|p| line(s, p));
    let section = cur.and_then(|i| placed.get(i)).and_then(|p| s.lyric_sheet.section_of(p.line));
    Prompt { section, current, singing, next }
}

/// The playhead the prompt follows: the transport while playing, else the insertion point.
fn playhead(app: &SoundApp) -> Samples {
    let e = &app.engine;
    if e.transport.playing { e.transport.position } else { e.session().edit.selection.start }
}

/// Draws the prompt over the whole of `ui`.
pub(crate) fn show(app: &mut SoundApp, ui: &mut Ui) {
    let r = ui.max_rect();
    let p = ui.painter().with_clip_rect(r);
    p.rect_filled(r, 0.0, Color32::BLACK);
    let at = playhead(app);
    let pr = prompt(app.engine.session(), at);
    if app.engine.transport.playing {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    let w = r.width().max(100.0);
    let big = (w / 18.0).clamp(18.0, 64.0);
    let pad = (w * 0.05).clamp(12.0, 60.0);
    let cy = r.center().y - big * 0.4;
    if let Some(sec) = &pr.section {
        p.text(pos2(r.min.x + pad, r.min.y + pad * 0.6), Align2::LEFT_TOP, sec.to_uppercase(), bold((big * 0.35).max(11.0)), Color32::from_gray(150));
    }
    match &pr.current {
        Some((n, text, k)) => {
            let col = if pr.singing { Color32::WHITE } else { Color32::from_gray(170) };
            let job = egui::text::LayoutJob::simple(text.clone(), bold(big), col, w - pad * 2.0 - big);
            let g = ui.fonts_mut(|f| f.layout_job(job));
            let top = cy - g.size().y / 2.0;
            p.text(pos2(r.min.x + pad, top + big * 0.55), Align2::LEFT_CENTER, n.to_string(), regular(big * 0.4), Color32::from_gray(110));
            let x = r.min.x + pad + big;
            let h = g.size().y;
            p.galley(pos2(x, top), g, col);
            // How far through the line the playhead is.
            let bar = Rect::from_min_size(pos2(x, top + h + big * 0.25), vec2(w - pad * 2.0 - big, (big * 0.08).max(3.0)));
            p.rect_filled(bar, 2.0, Color32::from_gray(40));
            if pr.singing {
                p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * k, bar.height())), 2.0, Color32::from_rgb(255, 200, 60));
            }
            if let Some((m, next)) = &pr.next {
                let ny = bar.max.y + big * 0.9;
                p.text(pos2(r.min.x + pad, ny), Align2::LEFT_CENTER, m.to_string(), regular(big * 0.3), Color32::from_gray(90));
                p.text(pos2(x, ny), Align2::LEFT_CENTER, next, regular(big * 0.6), Color32::from_gray(140));
            }
        }
        None => {
            let msg = if app.engine.session().lyric_sheet.placed.is_empty() {
                "Place lyric lines on the Lyrics ruler to follow them here."
            } else {
                "End of the placed lyrics."
            };
            p.text(r.center(), Align2::CENTER_CENTER, msg, regular((big * 0.4).max(13.0)), Color32::from_gray(140));
        }
    }
}

/// Draws the Teleprompter in its own native window.
pub fn show_separate(app: &mut SoundApp, ctx: &egui::Context) {
    let title = format!("{}  \u{2014}  Teleprompter", app.engine.session().name);
    let mut builder = egui::ViewportBuilder::default().with_title(title).with_min_inner_size([320.0, 180.0]).with_resizable(true);
    match app.ui.teleprompter_window.filter(|r| r.iter().all(|v| v.is_finite()) && r[2] >= 100.0 && r[3] >= 100.0) {
        Some([x, y, w, h]) => builder = builder.with_position([x, y]).with_inner_size([w, h]),
        None => builder = builder.with_inner_size([900.0, 420.0]),
    }
    ctx.show_viewport_immediate(viewport(), builder, |ui, _class| {
        ui.set_style(ui.ctx().global_style());
        if ui.ctx().viewport_id() != ViewportId::ROOT {
            crate::shortcuts::handle(app, ui.ctx());
            let info = ui.ctx().input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
            if let (Some(outer), Some(inner)) = info {
                app.ui.teleprompter_window = Some([outer.min.x, outer.min.y, inner.width(), inner.height()]);
            }
        }
        show(app, ui);
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            app.ui.teleprompter_separate = false;
            app.ui.show_teleprompter = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::SheetLine;

    fn session() -> Session {
        let mut s = Session::default();
        let texts = ["[Chorus]", "Jesus, the only one", "Jesus, the only one", "Nothing else"];
        s.lyric_sheet.lines = texts.iter().enumerate().map(|(i, t)| SheetLine::new(10 + i as u64, *t)).collect();
        s.lyric_sheet.placed = vec![LyricPlace { line: 11, start: 100, end: 200 }, LyricPlace { line: 12, start: 300, end: 400 }];
        s
    }

    #[test]
    fn follows_the_playhead_through_placed_lines() {
        let s = session();
        let p = prompt(&s, 150);
        assert!(p.singing);
        assert_eq!(p.current, Some((1, "Jesus, the only one".into(), 0.5)));
        assert_eq!(p.next, Some((2, "Jesus, the only one".into())));
        assert_eq!(p.section.as_deref(), Some("Chorus"));
        // Between lines: the next one waits, not yet lit.
        let p = prompt(&s, 250);
        assert!(!p.singing);
        assert_eq!(p.current.map(|c| c.0), Some(2));
        assert_eq!(p.next, None);
        // Past the end: nothing.
        assert_eq!(prompt(&s, 500).current, None);
        assert_eq!(prompt(&Session::default(), 0).current, None);
    }
}
