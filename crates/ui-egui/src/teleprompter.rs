//! The Teleprompter (Window › Transport & Session › Teleprompter): a big, high-contrast lyric view for the singer. It
//! follows the playhead through the lines placed on the Lyrics ruler: the current line large and
//! bright with a bar showing how far through it the playhead is, the next line below it, the
//! section above. It docks as a tab, or lives in its own native window (Window › Transport & Session › Teleprompter in
//! Separate Window), sized and placed as last left; closing that window docks it back. The lines'
//! chords are left out unless Show chords is on (right-click): then each stands above its syllable.

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
    /// The current and next lines' chords: (name, byte offset in the shown text it stands before).
    pub current_chords: Vec<(String, usize)>,
    pub next_chords: Vec<(String, usize)>,
}

/// A placed line as shown: its number, its words without chords, and the chords.
fn line_and_chords(s: &Session, p: &LyricPlace) -> Option<(u32, String, Vec<(String, usize)>)> {
    let sheet = &s.lyric_sheet;
    let st = soundcraft_model::strip_chords(&sheet.lines.iter().find(|l| l.id == p.line)?.text);
    let lead = st.text.len() - st.text.trim_start().len();
    let text = st.text.trim().to_string();
    let chords = st.chords.into_iter().map(|(n, at)| (n, at.saturating_sub(lead).min(text.len()))).collect();
    Some((sheet.number_of(p.line)?, text, chords))
}

fn line(s: &Session, p: &LyricPlace) -> Option<(u32, String)> {
    line_and_chords(s, p).map(|(n, t, _)| (n, t))
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
    let chords = |k: Option<usize>| k.and_then(|i| placed.get(i)).and_then(|p| line_and_chords(s, p)).map(|l| l.2).unwrap_or_default();
    Prompt { section, current, singing, next, current_chords: chords(cur), next_chords: chords(cur.map(|i| i + 1)) }
}

/// Lays out `text` for the prompt; with chords, every row has room above its words for them
/// (the words sit at the top of a taller row, the chords in the room the row above leaves), and
/// the chords' positions come back: (name, where its text starts, relative to the galley).
fn prompt_galley(
    ui: &Ui,
    text: &str,
    chords: &[(String, usize)],
    size: f32,
    bold_font: bool,
    col: Color32,
    wrap: f32,
) -> (std::sync::Arc<egui::Galley>, f32, Vec<(String, egui::Pos2)>) {
    let font = if bold_font { bold(size) } else { regular(size) };
    let room = if chords.is_empty() { 0.0 } else { (size * 0.62).round() };
    let row = ui.fonts_mut(|f| f.row_height(&font));
    let mut job = egui::text::LayoutJob::default();
    let fmt = egui::text::TextFormat { font_id: font, color: col, line_height: (room > 0.0).then_some(row + room), ..Default::default() };
    job.append(text, 0.0, fmt);
    job.wrap.max_width = wrap;
    let g = ui.fonts_mut(|f| f.layout_job(job));
    let mut out: Vec<(String, egui::Pos2)> = Vec::new();
    for (name, at) in chords {
        let ci = text.get(..*at).map_or(0, |p| p.chars().count());
        let r = g.pos_from_cursor(egui::text::CCursor::new(ci));
        let mut x = r.min.x;
        // Two chords over one word: the second one after the first.
        if let Some((prev, p)) = out.last()
            && (p.y - (r.min.y - room)).abs() < 1.0
        {
            x = x.max(p.x + (prev.chars().count() as f32 + 1.0) * room * 0.62);
        }
        out.push((name.clone(), pos2(x, r.min.y - room)));
    }
    (g, room, out)
}

/// The playhead the prompt follows: the transport while playing, else the insertion point.
fn playhead(app: &SoundApp) -> Samples {
    let e = &app.engine;
    if e.transport.playing { e.transport.position } else { e.session().edit.selection.start }
}

/// The colour chords are shown in on the prompt.
const CHORD_PROMPT: Color32 = Color32::from_rgb(255, 200, 60);

/// Draws the prompt over the whole of `ui`; right-click for its options.
pub(crate) fn show(app: &mut SoundApp, ui: &mut Ui) {
    let r = ui.max_rect();
    ui.interact(r, ui.id().with("teleprompter-options"), egui::Sense::click()).context_menu(|ui| {
        ui.checkbox(&mut app.ui.teleprompter_chords, "Show chords")
            .on_hover_text("Each line's chords ([Am] in the lyric sheet) above the words they fall on");
    });
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
    let show_chords = app.ui.teleprompter_chords;
    let none: Vec<(String, usize)> = Vec::new();
    match &pr.current {
        Some((n, text, k)) => {
            let col = if pr.singing { Color32::WHITE } else { Color32::from_gray(170) };
            let cur_chords = if show_chords { &pr.current_chords } else { &none };
            let (g, room, chords) = prompt_galley(ui, text, cur_chords, big, true, col, w - pad * 2.0 - big);
            let top = cy - g.size().y / 2.0 + room;
            p.text(pos2(r.min.x + pad, top + big * 0.55), Align2::LEFT_CENTER, n.to_string(), regular(big * 0.4), Color32::from_gray(110));
            let x = r.min.x + pad + big;
            // The last row's room for chords is not needed below it.
            let h = g.size().y - room;
            p.galley(pos2(x, top), g, col);
            for (name, at) in chords {
                p.text(pos2(x, top) + at.to_vec2(), Align2::LEFT_TOP, name, bold(room.max(8.0)), CHORD_PROMPT);
            }
            // How far through the line the playhead is.
            let bar = Rect::from_min_size(pos2(x, top + h + big * 0.25), vec2(w - pad * 2.0 - big, (big * 0.08).max(3.0)));
            p.rect_filled(bar, 2.0, Color32::from_gray(40));
            if pr.singing {
                p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * k, bar.height())), 2.0, Color32::from_rgb(255, 200, 60));
            }
            if let Some((m, next)) = &pr.next {
                let ny = bar.max.y + big * 0.9;
                p.text(pos2(r.min.x + pad, ny), Align2::LEFT_CENTER, m.to_string(), regular(big * 0.3), Color32::from_gray(90));
                if show_chords && !pr.next_chords.is_empty() {
                    let (g, room, chords) = prompt_galley(ui, next, &pr.next_chords, big * 0.6, false, Color32::from_gray(140), f32::INFINITY);
                    let first = g.rows.first().map_or(big * 0.6, |r| r.rect().height() - room);
                    let at = pos2(x, ny - first / 2.0);
                    p.galley(at, g, Color32::from_gray(140));
                    for (name, c) in chords {
                        p.text(at + c.to_vec2(), Align2::LEFT_TOP, name, bold(room.max(8.0)), CHORD_PROMPT.gamma_multiply(0.7));
                    }
                } else {
                    p.text(pos2(x, ny), Align2::LEFT_CENTER, next, regular(big * 0.6), Color32::from_gray(140));
                }
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
        let texts = ["{Chorus}", "[G]Jesus, the [D/F#]only one", "[Em] [C]", "Jesus, the only one", "Nothing else"];
        s.lyric_sheet.lines = texts.iter().enumerate().map(|(i, t)| SheetLine::new(10 + i as u64, *t)).collect();
        s.lyric_sheet.placed = vec![LyricPlace { line: 11, start: 100, end: 200 }, LyricPlace { line: 13, start: 300, end: 400 }];
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
        // The words without their chords; the chords where they stand in them.
        assert_eq!(p.current_chords, vec![("G".to_string(), 0), ("D/F#".to_string(), 11)]);
        assert!(p.next_chords.is_empty());
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
