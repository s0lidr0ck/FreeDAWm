//! The Lyrics tab (Window › Lyrics): a plain text editor bound to the session's lyric sheet. Each
//! lyric line gets a dim number in the gutter; the right margin shows its syllable count and a
//! rhyme letter (lines whose end words rhyme share a letter and colour, and the rhyme word is
//! underlined in it). "[Verse 1]" lines are section headings. Right-click a line for rhyme
//! suggestions. Typing is one undo step per line (Enter ends a step).

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use crate::word_finder;
use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, Rect, Stroke, Ui, pos2};
use serde_json::{Value, json};
use soundcraft_engine::lyric_text::{LineInfo, analyse, suggest_rhymes};
use soundcraft_engine::lyric_tools::{LineFit, MarkKind, WordMark, find_phrase, line_fit, rhyme_marks, stress_text, word_spans};
use std::sync::Arc;

/// Gutter (line numbers) and right margin (syllables, rhyme letter, placed dot) widths.
const GUTTER: f32 = 48.0;
const MARGIN: f32 = 132.0;
/// The optional stress column (left of the margin).
const STRESS_W: f32 = 120.0;
/// A fit flag's colour.
pub const FIT_FLAG: Color32 = Color32::from_rgb(240, 150, 40);
const LINE_FONT: f32 = 15.0;

/// The editor's text between frames, and the sheet text it was last in step with.
#[derive(Clone, Default)]
struct Buffer {
    text: String,
    lines: usize,
}

fn buffer_key() -> egui::Id {
    egui::Id::new("soundcraft-lyrics-buffer")
}

/// The sheet text analysed: per-line counts and letters, rhyming words. Kept between frames for
/// the same text (and dictionary state).
#[derive(Clone, Default)]
struct Analysis {
    key: (u64, bool),
    infos: Arc<Vec<LineInfo>>,
    marks: Arc<Vec<WordMark>>,
    /// Per sheet line, its fit (lyric lines only).
    fit: Arc<Vec<Option<LineFit>>>,
}

fn analysis(ctx: &egui::Context, text: &str) -> Analysis {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    let key = (h.finish(), soundcraft_engine::lexicon::ready());
    let id = egui::Id::new("soundcraft-lyrics-analysis");
    if let Some(a) = ctx.data(|d| d.get_temp::<Analysis>(id)).filter(|a| a.key == key) {
        return a;
    }
    let n = text.split('\n').count();
    let mut fit: Vec<Option<LineFit>> = vec![None; n];
    for f in line_fit(text.split('\n')) {
        let i = f.line;
        if let Some(slot) = fit.get_mut(i) {
            *slot = Some(f);
        }
    }
    let a = Analysis { key, infos: Arc::new(analyse(text.split('\n'))), marks: Arc::new(rhyme_marks(text.split('\n'))), fit: Arc::new(fit) };
    ctx.data_mut(|d| d.insert_temp(id, a.clone()));
    a
}

/// The colour of a rhyme type.
pub fn mark_color(k: MarkKind) -> Color32 {
    match k {
        MarkKind::Perfect => Color32::from_rgb(91, 192, 235),
        MarkKind::Slant => Color32::from_rgb(232, 206, 90),
        MarkKind::Multi => Color32::from_rgb(195, 143, 255),
        MarkKind::Internal => Color32::from_rgb(255, 143, 177),
    }
}

/// The colour of rhyme letter `c`.
pub fn rhyme_color(c: char) -> Color32 {
    const COLORS: [Color32; 8] = [
        Color32::from_rgb(240, 172, 60),
        Color32::from_rgb(96, 150, 255),
        Color32::from_rgb(110, 200, 110),
        Color32::from_rgb(232, 110, 170),
        Color32::from_rgb(170, 130, 240),
        Color32::from_rgb(80, 200, 200),
        Color32::from_rgb(230, 220, 90),
        Color32::from_rgb(240, 110, 90),
    ];
    let i = if c.is_ascii_uppercase() {
        usize::from(c as u8 - b'A')
    } else if c.is_ascii_lowercase() {
        usize::from(c as u8 - b'a')
    } else {
        0
    };
    COLORS.get(i % COLORS.len()).copied().unwrap_or(Color32::GRAY)
}

/// The editor's layout: headings bold and dim, lyric lines plain with each rhyming word tinted and
/// underlined in its rhyme type's colour (of the types in `kinds`; none when `rhymes` is off).
/// Highlighted runs (the hook, or a word picked in the Words list) are boxed in the accent colour.
fn layout(
    text: &str,
    infos: &[LineInfo],
    marks: &[WordMark],
    rhymes: bool,
    kinds: u8,
    highlights: &[(usize, usize, usize)],
    t: &Tokens,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = TextFormat { font_id: regular(LINE_FONT), color: t.text, ..TextFormat::default() };
    let heading = TextFormat { font_id: bold(12.0), color: t.text_dim, extra_letter_spacing: 0.5, ..TextFormat::default() };
    let mut lines = text.split('\n').peekable();
    let mut i = 0;
    while let Some(line) = lines.next() {
        let info = infos.get(i);
        let base = if info.is_some_and(|i| i.heading) { heading.clone() } else { plain.clone() };
        // The line's coloured runs: rhyming words (by type) and highlights.
        let words: Vec<(usize, usize, MarkKind)> =
            if rhymes { marks.iter().filter(|m| m.line == i).filter_map(|m| Some((m.start, m.end, m.shown(kinds)?))).collect() } else { Vec::new() };
        let lit: Vec<(usize, usize)> = highlights.iter().filter(|h| h.0 == i).map(|h| (h.1, h.2)).collect();
        let mut cuts: Vec<usize> = vec![0, line.len()];
        for (s, e, _) in &words {
            cuts.extend([*s, *e]);
        }
        for (s, e) in &lit {
            cuts.extend([*s, *e]);
        }
        cuts.retain(|c| *c <= line.len() && line.is_char_boundary(*c));
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let Some(piece) = line.get(a..b) else { continue };
            let mut fmt = base.clone();
            if let Some((_, _, k)) = words.iter().find(|(s, e, _)| *s <= a && b <= *e) {
                let c = mark_color(*k);
                fmt.underline = Stroke::new(1.5, c);
                fmt.background = c.gamma_multiply(0.18);
            }
            if lit.iter().any(|(s, e)| *s <= a && b <= *e) {
                fmt.background = t.accent.gamma_multiply(0.55);
                fmt.color = Color32::WHITE;
            }
            job.append(piece, 0.0, fmt);
        }
        if lines.peek().is_some() {
            job.append("\n", 0.0, base);
        }
        i += 1;
    }
    job
}

/// What the Lyrics tab highlights: the hook's occurrences, and/or a word from the Words list.
#[derive(Clone, Default, PartialEq)]
struct Highlight {
    hook: bool,
    word: Option<String>,
}

fn highlight_key() -> egui::Id {
    egui::Id::new("soundcraft-lyrics-highlight")
}

/// The runs to highlight in `text` for `h`.
fn highlight_runs(text: &str, hook: &str, h: &Highlight) -> Vec<(usize, usize, usize)> {
    let mut v = Vec::new();
    if h.hook {
        v.extend(find_phrase(text.split('\n'), hook));
    }
    if let Some(w) = &h.word {
        v.extend(find_phrase(text.split('\n'), w));
    }
    v
}

/// The toolbar's repetition check: the hook counter (click to highlight, set or clear the hook)
/// and the Words list (most repeated words; click one to highlight it).
fn repetition_menus(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let hook = soundcraft_engine::cmd::lyric_check::hook_of(&app.engine);
    let marked = app.engine.session().lyric_sheet.hook.is_some();
    let rep = soundcraft_engine::lyric_tools::repetition(app.engine.session().lyric_sheet.lines.iter().map(|l| l.text.as_str()), &hook);
    let count = rep.hits.len();
    let mut h: Highlight = ui.ctx().data(|d| d.get_temp(highlight_key())).unwrap_or_default();
    let before = h.clone();
    let label = if hook.is_empty() { "Hook".to_string() } else { format!("Hook \u{00D7}{count}") };
    let tip = if hook.is_empty() {
        "Mark the hook (title) phrase: select it in the lyrics and right-click \u{203A} Mark as Hook, or type it here".to_string()
    } else {
        format!("\u{2018}{hook}\u{2019} is sung {count} time{}", if count == 1 { "" } else { "s" })
    };
    ui.menu_button(label, |ui| {
        ui.label(egui::RichText::new(&tip).color(t.text_dim));
        let edit_id = egui::Id::new("soundcraft-lyrics-hook-edit");
        let mut phrase: String = ui.ctx().data(|d| d.get_temp(edit_id)).unwrap_or_else(|| hook.clone());
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut phrase).desired_width(180.0).hint_text("the hook phrase"));
            if ui.button("Set").clicked() {
                app.engine.end_merge();
                let _ = app.run("lyrics.hook_set", json!({"phrase": phrase}));
                h.hook = true;
                ui.close();
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(edit_id, phrase));
        ui.checkbox(&mut h.hook, "Highlight it in the lyrics");
        if marked && ui.button("Use the song name").clicked() {
            let _ = app.run("lyrics.hook_set", json!({"phrase": ""}));
            ui.close();
        }
    })
    .response
    .on_hover_text(tip);
    let words = rep.words;
    ui.menu_button("Words", |ui| {
        if words.is_empty() {
            ui.label(egui::RichText::new("No word is repeated yet").color(t.text_dim));
        }
        for (word, n) in words.iter().take(20) {
            let on = h.word.as_deref() == Some(word.as_str());
            if ui.selectable_label(on, format!("{word}  \u{00D7}{n}")).on_hover_text("Highlight it in the lyrics").clicked() {
                h.word = if on { None } else { Some(word.clone()) };
            }
        }
    })
    .response
    .on_hover_text("The most repeated words (not counting little words like \u{2018}the\u{2019})");
    if h != before {
        ui.ctx().data_mut(|d| d.insert_temp(highlight_key(), h));
    }
}

/// The rhyme-type toggles and their legend: a coloured chip per type.
fn rhyme_toggles(app: &mut SoundApp, ui: &mut Ui, t: &Tokens) {
    for k in MarkKind::ALL {
        let on = app.ui.lyrics_rhyme_kinds & k.bit() != 0;
        let c = mark_color(k);
        let text = egui::RichText::new(k.name()).size(11.5).color(if on { t.text } else { t.text_dim.gamma_multiply(0.6) });
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::click());
        ui.painter().rect_filled(rect, 2.0, if on { c } else { c.gamma_multiply(0.25) });
        let label = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
        let tip = match k {
            MarkKind::Perfect => "Perfect rhymes: end words with the same rhyme sound (long / song)",
            MarkKind::Slant => "Slant rhymes: end words that nearly rhyme (home / alone)",
            MarkKind::Multi => "Multi-syllable rhymes: two or more trailing syllables match (falling / calling)",
            MarkKind::Internal => "Internal rhymes: inside a line, or mid-line across neighbouring lines",
        };
        if resp.on_hover_text(tip).clicked() | label.on_hover_text(tip).clicked() {
            app.ui.lyrics_rhyme_kinds ^= k.bit();
        }
        ui.add_space(4.0);
    }
}

pub(crate) fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    ui.painter().rect_filled(ui.max_rect(), 0.0, t.panel_bg);
    ui.add_space(6.0);
    // The tools sit at the right of the toggles, or on a row of their own when the tab is narrow.
    let wide = ui.available_width() >= 1150.0;
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("syllables").color(t.text_dim));
        if ui.button(if app.ui.lyrics_syllables { "on" } else { "off" }).on_hover_text("Show each line's syllable count").clicked() {
            app.ui.lyrics_syllables = !app.ui.lyrics_syllables;
        }
        ui.add_space(6.0);
        ui.add_space(6.0);
        ui.label(egui::RichText::new("stress").color(t.text_dim));
        if ui.button(if app.ui.lyrics_stress { "on" } else { "off" }).on_hover_text("Show each line's stress pattern (\u{25CF} stressed, \u{25CB} not)").clicked() {
            app.ui.lyrics_stress = !app.ui.lyrics_stress;
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new("fit").color(t.text_dim));
        if ui
            .button(if app.ui.lyrics_fit { "on" } else { "off" })
            .on_hover_text("Flag lines whose syllables or stresses differ from the same line of the section's first take (Verse 1 for Verse 2, the first Chorus for the others)")
            .clicked()
        {
            app.ui.lyrics_fit = !app.ui.lyrics_fit;
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new("rhyme").color(t.text_dim));
        let rl = if app.ui.lyrics_rhymes { "on" } else { "off" };
        egui::ComboBox::from_id_salt("lyrics-rhyme").selected_text(rl).width(56.0).show_ui(ui, |ui| {
            if ui.selectable_label(app.ui.lyrics_rhymes, "on").clicked() {
                app.ui.lyrics_rhymes = true;
            }
            if ui.selectable_label(!app.ui.lyrics_rhymes, "off").clicked() {
                app.ui.lyrics_rhymes = false;
            }
        });
        if app.ui.lyrics_rhymes {
            ui.add_space(4.0);
            rhyme_toggles(app, ui, &t);
        }
        if wide {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| tools(app, ui, &t));
        }
    });
    if !wide {
        ui.horizontal(|ui| ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| tools(app, ui, &t)));
    }
    ui.add_space(4.0);
    let sheet_text = app.engine.session().lyric_sheet.text();
    let mut buf: Buffer = ui.ctx().data(|d| d.get_temp(buffer_key())).unwrap_or_default();
    let editor_id = ui.id().with("lyrics-editor");
    let focused = ui.ctx().memory(|m| m.has_focus(editor_id));
    // Undo, a command or another window changed the sheet: show it (not while typing).
    if !focused && buf.text != sheet_text {
        buf.lines = sheet_text.matches('\n').count();
        buf.text = sheet_text;
    }
    let (syl, rhymes, kinds) = (app.ui.lyrics_syllables, app.ui.lyrics_rhymes, app.ui.lyrics_rhyme_kinds);
    // Each row's sheet line (id, placed, whose range is the selection), while the editor shows
    // the sheet as it is.
    let rows: Vec<(u64, bool, bool)> = {
        let s = app.engine.session();
        let sel = s.edit.selection;
        if buf.text == s.lyric_sheet.text() {
            s.lyric_sheet
                .lines
                .iter()
                .map(|l| {
                    let q = s.lyric_sheet.place_of(l.id);
                    (l.id, q.is_some(), q.is_some_and(|q| q.start == sel.start && q.end == sel.end))
                })
                .collect()
        } else {
            Vec::new()
        }
    };
    // Each row's alternates: (versions, active).
    let alts: Vec<(usize, u32)> =
        if rows.is_empty() { Vec::new() } else { app.engine.session().lyric_sheet.lines.iter().map(|l| (l.alts.len(), l.active)).collect() };
    let mut switch: Option<(u64, i64)> = None;
    let jump: Option<u64> = ui.ctx().data_mut(|d| d.remove_temp(crate::lyric_ruler::jump_key()));
    let (stress_col, fit_on) = (app.ui.lyrics_stress, app.ui.lyrics_fit);
    let margin = MARGIN + if stress_col { STRESS_W } else { 0.0 };
    let hl: Highlight = ui.ctx().data(|d| d.get_temp(highlight_key())).unwrap_or_default();
    let hook = soundcraft_engine::cmd::lyric_check::hook_of(&app.engine);
    let first = analysis(ui.ctx(), &buf.text);
    let (mut infos, mut fits) = (first.infos, first.fit);
    egui::ScrollArea::vertical().id_salt("lyrics-scroll").auto_shrink([false, false]).show(ui, |ui| {
        let width = ui.available_width();
        let tk = t;
        let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap: f32| {
            let s = text.as_str();
            let a = analysis(ui.ctx(), s);
            let lit = highlight_runs(s, &hook, &hl);
            let mut job = layout(s, &a.infos, &a.marks, rhymes, kinds, &lit, &tk);
            job.wrap.max_width = wrap;
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let left = ui.max_rect().min.x;
        let right = left + width;
        let out = ui
            .horizontal_top(|ui| {
                ui.add_space(GUTTER);
                egui::TextEdit::multiline(&mut buf.text)
                    .id(editor_id)
                    .frame(egui::Frame::NONE)
                    .desired_width((width - GUTTER - margin).max(80.0))
                    .desired_rows(12)
                    .lock_focus(true)
                    .hint_text("Write the lyrics here. A line like [Verse 1] starts a section.")
                    .layouter(&mut layouter)
                    .show(ui)
            })
            .inner;
        let r = &out.response;
        if r.changed() {
            let a = analysis(ui.ctx(), &buf.text);
            (infos, fits) = (a.infos, a.fit);
            let _ = app.engine.execute_merged("lyrics.sheet_set", &json!({"text": buf.text}), "lyric-sheet");
            // One undo step per line: Enter (or a pasted block) closes the step.
            let lines = buf.text.matches('\n').count();
            if lines != buf.lines {
                app.engine.end_merge();
                buf.lines = lines;
            }
        }
        if r.lost_focus() {
            app.engine.end_merge();
        }
        // Gutter numbers and the right margin, row by row.
        let p = ui.painter().with_clip_rect(ui.clip_rect());
        let mut hover_line: Option<usize> = None;
        let pointer = ui.ctx().pointer_hover_pos();
        let mut line = 0usize;
        for row in &out.galley.rows {
            let rr = row.rect().translate(out.galley_pos.to_vec2());
            let full = Rect::from_min_max(pos2(left, rr.min.y), pos2(right, rr.max.y));
            if pointer.is_some_and(|p| full.contains(p)) {
                hover_line = Some(line);
            }
            if let Some(info) = infos.get(line) {
                let cy = rr.center().y;
                let row_line = rows.get(line).copied();
                if let Some((id, _, selected)) = row_line {
                    if selected {
                        p.rect_filled(full, 0.0, t.accent.gamma_multiply(0.12));
                    }
                    if jump == Some(id) {
                        ui.scroll_to_rect(full, Some(egui::Align::Center));
                        p.rect_filled(full, 0.0, t.accent.gamma_multiply(0.25));
                    }
                }
                if let Some(n) = info.number {
                    // The number is a handle: drag it onto the Lyrics ruler; click selects its range.
                    if let Some((id, placed, _)) = row_line {
                        let handle = Rect::from_min_max(pos2(left, rr.min.y), pos2(left + GUTTER - 4.0, rr.max.y));
                        let h = ui.interact(handle, ui.id().with(("lyric-line", id)), egui::Sense::click_and_drag());
                        if h.drag_started() {
                            egui::DragAndDrop::set_payload(ui.ctx(), crate::lyric_ruler::LineDrag(id));
                        }
                        if h.dragged()
                            && let Some(pt) = ui.ctx().pointer_latest_pos()
                        {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            let fg = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("lyric-line-drag")));
                            let text = buf.text.split('\n').nth(line).unwrap_or("").trim().to_string();
                            fg.text(pt + egui::vec2(12.0, 0.0), Align2::LEFT_CENTER, format!("{n}  {text}"), regular(12.0), Color32::WHITE);
                        }
                        if h.clicked() && placed {
                            let _ = app.run("lyrics.select_line", json!({"line": id}));
                        }
                        let tip = if placed {
                            "Click: select its time range. Drag onto the Lyrics ruler to move it"
                        } else {
                            "Drag onto the Lyrics ruler to place it"
                        };
                        if h.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text(tip).hovered() {
                            p.rect_filled(handle.shrink(1.0), 3.0, t.button);
                        }
                        if placed {
                            p.circle_filled(pos2(right - 20.0, cy), 3.5, crate::lyric_ruler::EDGE_DOT);
                        }
                    }
                    p.text(pos2(left + GUTTER - 14.0, cy), Align2::RIGHT_CENTER, n.to_string(), regular(11.5), t.text_dim.gamma_multiply(0.7));
                    if syl {
                        p.text(pos2(right - 66.0, cy), Align2::RIGHT_CENTER, info.syllables.to_string(), regular(11.5), t.text_dim);
                    }
                    if let Some(Some(f)) = fits.get(line) {
                        if stress_col {
                            let step = ((STRESS_W - 12.0) / f.stress.len().max(1) as f32).clamp(3.0, 8.0);
                            let x0 = right - margin + 6.0;
                            for (k, s) in f.stress.iter().enumerate() {
                                let c = pos2(x0 + step * k as f32 + step * 0.5, cy);
                                if *s {
                                    p.circle_filled(c, (step * 0.4).min(3.2), t.text);
                                } else {
                                    p.circle_stroke(c, (step * 0.3).min(2.4), Stroke::new(1.0, t.text_dim));
                                }
                            }
                        }
                        let flag = f.flag.as_deref().filter(|_| fit_on);
                        if flag.is_some() {
                            p.circle_filled(pos2(right - 88.0, cy), 3.5, FIT_FLAG);
                        }
                        // The margin's tooltip: the stress pattern, and why the line is flagged.
                        let zone = Rect::from_min_max(pos2(right - 100.0, rr.min.y), pos2(right - 30.0, rr.max.y));
                        let column = Rect::from_min_max(pos2(right - margin, rr.min.y), pos2(right - margin + STRESS_W, rr.max.y));
                        let over = |r: Rect| pointer.is_some_and(|q| r.contains(q));
                        let zone = if stress_col && over(column) { column } else { zone };
                        if over(zone) {
                            let mut tip = format!("{} syllables: {}", f.syllables, stress_text(&f.stress));
                            if let (Some(n), Some(s)) = (f.partner_number, f.partner_section.as_deref()) {
                                tip.push_str(&format!("\nSung against line {n} ({s})"));
                            }
                            if let Some(why) = flag {
                                tip.push_str(&format!("\n\u{26A0} {why}"));
                            }
                            let zid = ui.id().with(("lyric-fit", line));
                            let zr = ui.interact(zone, zid, egui::Sense::hover());
                            zr.on_hover_text(tip);
                        }
                    }
                    if let (Some(c), true) = (info.rhyme, rhymes) {
                        let col = if info.rhymes { rhyme_color(c) } else { t.text_dim.gamma_multiply(0.6) };
                        p.text(pos2(right - 46.0, cy), Align2::CENTER_CENTER, c.to_string(), bold(11.5), col);
                    }
                    // The alternates switcher: "2/3"; click for the next version, right-click the previous.
                    if let (Some((id, _, _)), Some(&(n, active))) = (row_line, alts.get(line))
                        && n > 1
                    {
                        let rect = Rect::from_center_size(pos2(right - 116.0, cy), egui::vec2(30.0, rr.height().max(14.0) - 2.0));
                        let h = ui.interact(rect, ui.id().with(("lyric-alt", id)), egui::Sense::click());
                        p.rect_filled(rect, 3.0, if h.hovered() { t.button_hi } else { t.button });
                        p.text(rect.center(), Align2::CENTER_CENTER, format!("{}/{n}", active + 1), regular(11.0), t.text);
                        if h.clicked() {
                            switch = Some((id, 1));
                        } else if h.secondary_clicked() {
                            switch = Some((id, -1));
                        }
                        let versions = app.engine.session().lyric_sheet.lines.get(line).map(|l| l.alts.clone()).unwrap_or_default();
                        let list: Vec<String> = versions
                            .iter()
                            .enumerate()
                            .map(|(k, v)| format!("{}{}  {v}", if k as u32 == active { "\u{25B8} " } else { "   " }, k + 1))
                            .collect();
                        h.on_hover_text(format!("Alternate lines: click for the next, right-click the previous\n{}", list.join("\n")));
                    }
                }
            }
            if row.ends_with_newline {
                line += 1;
            }
        }
        // The word under the pointer or at the text cursor: the Word Finder's subject.
        let word_at = |char_idx: usize| -> Option<SheetWord> {
            let mut count = 0usize;
            for (li, l) in buf.text.split('\n').enumerate() {
                let n = l.chars().count();
                if char_idx <= count + n {
                    let byte = l.char_indices().nth(char_idx - count).map_or(l.len(), |(b, _)| b);
                    let (s, e) = word_spans(l).into_iter().find(|(s, e)| *s <= byte && byte <= *e)?;
                    return Some(SheetWord { line: li, start: s, end: e, word: l.get(s..e)?.to_string() });
                }
                count += n + 1;
            }
            None
        };
        let to_target = |w: &SheetWord| rows.get(w.line).map(|(id, _, _)| word_finder::Target { line: *id, start: w.start, end: w.end });
        let at_cursor = out.cursor_range.and_then(|c| word_at(c.primary.index.into()));
        let cursor_word = at_cursor.as_ref().map(|w| (w.word.clone(), to_target(w)));
        // The selected text (for Mark as Hook).
        let selected: Option<String> = out.cursor_range.and_then(|c| {
            let (a, b) = (usize::from(c.primary.index), usize::from(c.secondary.index));
            let (a, b) = (a.min(b), a.max(b));
            let s: String = buf.text.chars().skip(a).take(b - a).collect();
            let s = s.trim().to_string();
            (!s.is_empty() && !s.contains('\n') && s.chars().count() <= 80).then_some(s)
        });
        if r.secondary_clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("soundcraft-lyrics-menu-selection"), selected.clone()));
        }
        let menu_selection: Option<String> = ui.ctx().data(|d| d.get_temp(egui::Id::new("soundcraft-lyrics-menu-selection"))).flatten();
        // The lyric lines the cursor or selection is on (for Copy for AI).
        let sel_lines: Vec<u32> = out
            .cursor_range
            .map(|c| {
                let line_of = |ci: usize| buf.text.chars().take(ci).filter(|ch| *ch == '\n').count();
                let (a, b) = (line_of(usize::from(c.primary.index)), line_of(usize::from(c.secondary.index)));
                (a.min(b)..=a.max(b)).filter_map(|i| infos.get(i).and_then(|x| x.number)).collect()
            })
            .unwrap_or_default();
        if r.has_focus() {
            ui.ctx().data_mut(|d| d.insert_temp(selected_lines_key(), sel_lines));
        }
        ui.ctx().data_mut(|d| d.insert_temp(cursor_word_key(), cursor_word));
        // Right-click a line: find words for the word under the pointer; rhymes for its end word.
        let menu_key = egui::Id::new("soundcraft-lyrics-menu-line");
        if r.secondary_clicked() {
            let under = pointer.map(|p| out.galley.cursor_from_pos(p - out.galley_pos).index.into()).and_then(word_at);
            ui.ctx().data_mut(|d| d.insert_temp(menu_key, (hover_line, under)));
        }
        let (target, under): (Option<usize>, Option<SheetWord>) = ui.ctx().data(|d| d.get_temp(menu_key)).unwrap_or_default();
        let word = target.and_then(|i| {
            let info = infos.get(i)?;
            let (a, b) = info.word?;
            buf.text.split('\n').nth(i).and_then(|l| l.get(a..b)).map(str::to_string)
        });
        let menu_line: Option<(u64, usize)> = target.and_then(|i| {
            let (id, _, _) = rows.get(i)?;
            infos.get(i)?.number?;
            Some((*id, alts.get(i).map_or(0, |a| a.0)))
        });
        let mut alt_action: Option<(&'static str, Value)> = None;
        r.context_menu(|ui| {
            if let Some((id, n)) = menu_line {
                if ui.button("Add Alternate").on_hover_text("Another version of this line; switch with the 1/2 box in the margin").clicked() {
                    alt_action = Some(("lyrics.alt_add", json!({"line": id})));
                    ui.close();
                }
                if n > 1 {
                    if ui.button("Next Alternate").clicked() {
                        alt_action = Some(("lyrics.alt_switch", json!({"line": id, "step": 1})));
                        ui.close();
                    }
                    if ui.button("Delete This Alternate").clicked() {
                        alt_action = Some(("lyrics.alt_delete", json!({"line": id})));
                        ui.close();
                    }
                }
                ui.separator();
            }
            if let Some(s) = &menu_selection
                && ui.button(format!("Mark \u{2018}{s}\u{2019} as the Hook")).on_hover_text("The repetition check counts this phrase").clicked()
            {
                alt_action = Some(("lyrics.hook_set", json!({"phrase": s})));
                ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Highlight>(highlight_key()).hook = true);
                ui.close();
            }
            if let Some(w) = &under
                && ui
                    .button(format!("Find words for \u{2018}{}\u{2019}\u{2026}", w.word))
                    .on_hover_text("Rhymes, synonyms and the definition; click one to put it in the line")
                    .clicked()
            {
                word_finder::open(ui.ctx(), &w.word, to_target(w));
                ui.close();
            }
            end_word_menu(ui, word.as_deref());
        });
        if let Some((id, step)) = switch {
            alt_action = Some(("lyrics.alt_switch", json!({"line": id, "step": step})));
        }
        if let Some((cmd, p)) = alt_action {
            app.engine.end_merge();
            if let Err(e) = app.run(cmd, p) {
                app.ui.status = e;
            }
            ui.ctx().memory_mut(|m| m.surrender_focus(editor_id));
        }
    });
    drafts_compare(app, ui.ctx());
    ai_window(app, ui.ctx());
    word_finder::show(app, ui.ctx());
    ui.ctx().data_mut(|d| d.insert_temp(buffer_key(), buf));
}

/// A word of the editor's text: its line (index) and byte range there.
#[derive(Clone, Debug, Default)]
struct SheetWord {
    line: usize,
    start: usize,
    end: usize,
    word: String,
}

fn cursor_word_key() -> egui::Id {
    egui::Id::new("soundcraft-lyrics-cursor-word")
}

/// The right-click menu's rhyme suggestions for a line's end word.
fn end_word_menu(ui: &mut Ui, word: Option<&str>) {
    match word {
        Some(w) => {
            ui.menu_button(format!("Suggest rhymes for \u{2018}{w}\u{2019}"), |ui| {
                let list = suggest_rhymes(w, 30);
                if list.is_empty() {
                    ui.label(egui::RichText::new("No rhymes in the built-in list").color(Tokens::current().text_dim));
                }
                for s in list {
                    if ui.button(&s).on_hover_text("Copy").clicked() {
                        ui.ctx().copy_text(s.clone());
                        ui.close();
                    }
                }
            });
        }
        None => {
            ui.label(egui::RichText::new("Right-click a lyric line for rhymes").color(Tokens::current().text_dim));
        }
    }
}

/// The toolbar's tools, right to left: placing on the ruler, drafts, the repetition check, Copy for
/// AI, the Word Finder.
fn tools(app: &mut SoundApp, ui: &mut Ui, t: &Tokens) {
    ui.add_space(8.0);
    place_menu(app, ui);
    drafts_menu(app, ui);
    repetition_menus(app, ui);
    ai_button(app, ui);
    let at: Option<(String, Option<word_finder::Target>)> = ui.ctx().data(|d| d.get_temp(cursor_word_key())).flatten();
    if ui.button("Find Words\u{2026}").on_hover_text("The Word Finder for the word at the cursor: rhymes, synonyms, definition (offline)").clicked() {
        let (w, target) = at.unwrap_or_default();
        word_finder::open(ui.ctx(), &w, target);
    }
    // The dictionary loads in the background on first use; counts sharpen when it lands.
    if soundcraft_engine::lexicon::get().is_none() {
        ui.label(egui::RichText::new("loading dictionary\u{2026}").color(t.text_dim).italics());
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
    }
}

fn selected_lines_key() -> egui::Id {
    egui::Id::new("soundcraft-lyrics-selected-lines")
}

/// The Copy for AI window's state.
#[derive(Clone, Default)]
struct AiAsk {
    open: bool,
    lines: Vec<u32>,
    request: String,
}

fn ai_key() -> egui::Id {
    egui::Id::new("soundcraft-lyrics-ai")
}

/// "Copy for AI": opens a small window with the request (editable) for the lines at the cursor.
fn ai_button(app: &mut SoundApp, ui: &mut Ui) {
    if ui
        .button("Copy for AI\u{2026}")
        .on_hover_text("Copy a prompt with the lyrics, their structure, syllables, stress and rhymes and your request, to paste into any AI chat")
        .clicked()
    {
        let lines: Vec<u32> = ui.ctx().data(|d| d.get_temp(selected_lines_key())).unwrap_or_default();
        let request =
            app.run("lyrics.ai_prompt", json!({"lines": lines})).ok().and_then(|v| v["request"].as_str().map(str::to_string)).unwrap_or_default();
        ui.ctx().data_mut(|d| d.insert_temp(ai_key(), AiAsk { open: true, lines, request }));
    }
}

/// The Copy for AI window: the request to edit, the prompt to copy.
fn ai_window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut ask: AiAsk = ctx.data(|d| d.get_temp(ai_key())).unwrap_or_default();
    if !ask.open {
        return;
    }
    let t = Tokens::current();
    let mut open = true;
    egui::Window::new("Copy for AI").id(egui::Id::new("lyrics-ai-window")).open(&mut open).default_width(520.0).show(ctx, |ui| {
        let about = if ask.lines.is_empty() {
            "The whole song".to_string()
        } else {
            format!("Line{} {}", if ask.lines.len() == 1 { "" } else { "s" }, ask.lines.iter().map(u32::to_string).collect::<Vec<_>>().join(", "))
        };
        ui.label(egui::RichText::new(about).color(t.text_dim));
        ui.label(egui::RichText::new("Your request").font(bold(13.0)).color(t.text));
        ui.add(egui::TextEdit::multiline(&mut ask.request).desired_rows(3).desired_width(f32::INFINITY));
        let prompt = app
            .run("lyrics.ai_prompt", json!({"lines": ask.lines, "request": ask.request}))
            .ok()
            .and_then(|v| v["prompt"].as_str().map(str::to_string))
            .unwrap_or_default();
        ui.horizontal(|ui| {
            if ui.button("Copy prompt").on_hover_text("Then paste it into a chat with any AI").clicked() {
                ui.ctx().copy_text(prompt.clone());
                app.ui.status = format!("Copied the lyrics prompt ({} characters)", prompt.chars().count());
            }
            ui.label(egui::RichText::new("Nothing is sent anywhere: paste it into the chat you use.").size(11.0).color(t.text_dim));
        });
        egui::CollapsingHeader::new("Preview").show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                ui.label(egui::RichText::new(&prompt).monospace().size(11.0).color(t.text));
            });
        });
    });
    ask.open = open;
    ctx.data_mut(|d| d.insert_temp(ai_key(), ask));
}

fn compare_key() -> egui::Id {
    egui::Id::new("soundcraft-lyrics-compare-draft")
}

/// The Drafts menu: save the sheet as a draft (named), and per draft restore, compare or delete.
fn drafts_menu(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let drafts: Vec<(u64, String, String)> =
        app.engine.session().lyric_sheet.drafts.iter().rev().map(|d| (d.id, d.name.clone(), d.created.replace('T', " ").replace('Z', ""))).collect();
    ui.menu_button(format!("Drafts ({})", drafts.len()), |ui| {
        let name_id = egui::Id::new("soundcraft-lyrics-draft-name");
        let mut name: String = ui.ctx().data(|d| d.get_temp(name_id)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut name).desired_width(150.0).hint_text("name (optional)"));
            if ui.button("Save Draft").on_hover_text("Snapshot the whole sheet with a name and the time").clicked() {
                let p = if name.trim().is_empty() { json!({}) } else { json!({"name": name.trim()}) };
                app.engine.end_merge();
                if let Err(e) = app.run("lyrics.draft_save", p) {
                    app.ui.status = e;
                }
                name.clear();
                ui.close();
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(name_id, name));
        if drafts.is_empty() {
            ui.label(egui::RichText::new("No drafts yet").color(t.text_dim));
        }
        for (id, name, created) in drafts {
            ui.menu_button(format!("{name}   {created}"), |ui| {
                if ui.button("Restore").on_hover_text("Put this draft back as the sheet (Undo brings the current one back)").clicked() {
                    app.engine.end_merge();
                    if let Err(e) = app.run("lyrics.draft_restore", json!({"draft": id})) {
                        app.ui.status = e;
                    }
                    ui.close();
                }
                if ui.button("Compare").on_hover_text("The draft beside the current sheet, read-only").clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(compare_key(), Some(id)));
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    let _ = app.run("lyrics.draft_delete", json!({"draft": id}));
                    ui.close();
                }
            });
        }
    });
}

/// The draft comparison window: the draft and the current sheet side by side, read-only, lines
/// that differ tinted.
fn drafts_compare(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(id) = ctx.data(|d| d.get_temp::<Option<u64>>(compare_key())).flatten() else { return };
    let (draft, now) = {
        let sheet = &app.engine.session().lyric_sheet;
        (sheet.drafts.iter().find(|d| d.id == id).cloned(), sheet.text())
    };
    let Some(draft) = draft else {
        ctx.data_mut(|d| d.remove::<Option<u64>>(compare_key()));
        return;
    };
    let t = Tokens::current();
    let mut open = true;
    let then = draft.text();
    egui::Window::new(format!("Compare: {}", draft.name))
        .id(egui::Id::new("lyrics-compare-window"))
        .open(&mut open)
        .default_size([760.0, 480.0])
        .show(ctx, |ui| {
            let (a, b): (Vec<&str>, Vec<&str>) = (then.split('\n').collect(), now.split('\n').collect());
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.columns(2, |cols| {
                    for (col, (title, lines, other)) in
                        cols.iter_mut().zip([(format!("{} ({})", draft.name, draft.created), &a, &b), ("Now".to_string(), &b, &a)])
                    {
                        col.label(egui::RichText::new(title).font(bold(13.0)).color(t.text));
                        col.separator();
                        for l in lines.iter() {
                            let changed = !other.contains(l);
                            let text = egui::RichText::new(if l.is_empty() { " " } else { l }).font(regular(13.0));
                            let text = if crate::lyrics_panel::is_heading(l) {
                                text.color(t.text_dim)
                            } else if changed {
                                text.color(t.text).background_color(FIT_FLAG.gamma_multiply(0.18))
                            } else {
                                text.color(t.text)
                            };
                            col.label(text);
                        }
                    }
                });
            });
        });
    if !open {
        ctx.data_mut(|d| d.remove::<Option<u64>>(compare_key()));
    }
}

/// Whether a sheet line is a "[Section]" heading.
pub fn is_heading(line: &str) -> bool {
    soundcraft_model::heading_name(line).is_some()
}

/// "Place lines on ruler…": the next unplaced lines over the edit selection, spread evenly or at
/// the times Transcribe Lyrics found for their words.
fn place_menu(app: &mut SoundApp, ui: &mut Ui) {
    let (range, left) = {
        let s = app.engine.session();
        (s.edit.selection.end > s.edit.selection.start, soundcraft_engine::cmd::lyric_place::unplaced(s).len())
    };
    ui.menu_button("Place lines on ruler\u{2026}", |ui| {
        if !range {
            ui.label(egui::RichText::new("Select a time range first").color(Tokens::current().text_dim));
        }
        for (label, count) in [
            (format!("All {left} unplaced lines, evenly"), None),
            ("Next 2 lines, evenly".to_string(), Some(2)),
            ("Next 4 lines, evenly".to_string(), Some(4)),
            ("Next 8 lines, evenly".to_string(), Some(8)),
        ] {
            if ui.add_enabled(range && left > 0, egui::Button::new(label)).clicked() {
                show_ruler(app);
                let _ = app.run("lyrics.place_lines", json!({"count": count}));
                ui.close();
            }
        }
        ui.separator();
        if ui
            .add_enabled(left > 0, egui::Button::new("From transcribed words"))
            .on_hover_text("Lines take the times Transcribe Lyrics found for their words")
            .clicked()
        {
            show_ruler(app);
            let _ = app.run("lyrics.place_lines", json!({"from": "transcribed"}));
            ui.close();
        }
    });
}

/// Shows the Lyrics ruler (placing a line makes it visible).
fn show_ruler(app: &mut SoundApp) {
    if !app.engine.session().edit.rulers.iter().any(|r| r == "lyrics") {
        let _ = app.run("view.ruler", json!({"ruler": "lyrics", "visible": true}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_underlines_shared_rhyme_words_only() {
        let text = "[Verse 1]\nWhen the night was long\nYou were there to sing the song\nNo rhyme here";
        let infos = analyse(text.split('\n'));
        let marks = rhyme_marks(text.split('\n'));
        let job = layout(text, &infos, &marks, true, 0x0f, &[], &Tokens::current());
        let underlined: Vec<&str> = job
            .sections
            .iter()
            .filter(|s| s.format.underline.width > 0.0)
            .filter_map(|s| job.text.get(s.byte_range.start.0..s.byte_range.end.0))
            .collect();
        assert_eq!(underlined, vec!["long", "song"]);
        assert_eq!(job.text, text);
        let off = layout(text, &infos, &marks, false, 0x0f, &[], &Tokens::current());
        assert!(off.sections.iter().all(|s| s.format.underline.width == 0.0));
        let none = layout(text, &infos, &marks, true, MarkKind::Slant.bit(), &[], &Tokens::current());
        assert!(none.sections.iter().all(|s| s.format.underline.width == 0.0));
        assert_eq!(none.text, text);
        // A highlighted hook run is boxed; the text is unchanged.
        let lit = highlight_runs(text, "sing the song", &Highlight { hook: true, word: None });
        assert_eq!(lit, vec![(2, 18, 31)]);
        let job = layout(text, &infos, &marks, true, 0x0f, &lit, &Tokens::current());
        assert_eq!(job.text, text);
        let boxed: String = job
            .sections
            .iter()
            .filter(|s| s.format.color == Color32::WHITE)
            .filter_map(|s| job.text.get(s.byte_range.start.0..s.byte_range.end.0))
            .collect();
        assert_eq!(boxed, "sing the song");
        assert_ne!(rhyme_color('A'), rhyme_color('B'));
    }
}
