//! The Lyrics tab's Word Finder: a floating window for one word, with Rhymes (perfect, multi,
//! slant; grouped by syllable count, filtered by stress pattern), Synonyms (and related words) and
//! Definition tabs, offline from the bundled cmudict and WordNet. Opened from a word of the sheet
//! (right-click › Find Words, or the toolbar with the cursor in a word), a click on a suggestion
//! replaces that word (`lyrics.replace_word`, one undo step); opened on a typed word, it copies.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Ui};
use serde_json::json;
use soundcraft_engine::lexicon::{self, Found, Related, RhymeKind, Sense, WordLookup};
use std::sync::Arc;

/// The word of the sheet the finder replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// The sheet line id.
    pub line: u64,
    /// The word's byte range in the line.
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Rhymes,
    Synonyms,
    Definition,
}

/// The finder's state (kept in egui's memory between frames).
#[derive(Clone, Default)]
pub struct Finder {
    pub open: bool,
    pub word: String,
    pub target: Option<Target>,
    pub tab: Tab,
    /// Only rhymes with this stress pattern ("DUM-da"); `None`: any.
    pub stress: Option<String>,
    /// The word field while it is being edited.
    query: String,
}

/// What the finder shows for a word (looked up once per word).
#[derive(Default)]
struct Results {
    word: String,
    rhymes: Vec<Found>,
    related: Related,
    senses: Vec<Sense>,
}

fn key() -> egui::Id {
    egui::Id::new("soundcraft-word-finder")
}

/// Opens the finder on `word` (replacing it at `target`, if given).
pub fn open(ctx: &egui::Context, word: &str, target: Option<Target>) {
    ctx.data_mut(|d| {
        let f: &mut Finder = d.get_temp_mut_or_default(key());
        f.open = true;
        f.word = word.to_string();
        f.query = word.to_string();
        f.target = target;
        f.stress = None;
    });
}

/// The finder's state now.
pub fn state(ctx: &egui::Context) -> Finder {
    ctx.data(|d| d.get_temp::<Finder>(key())).unwrap_or_default()
}

fn results(ctx: &egui::Context, word: &str) -> Option<Arc<Results>> {
    let lex = lexicon::get()?;
    let id = egui::Id::new("soundcraft-word-finder-results");
    if let Some(r) = ctx.data(|d| d.get_temp::<Arc<Results>>(id)).filter(|r| r.word == word) {
        return Some(r);
    }
    let r = Arc::new(Results { word: word.to_string(), rhymes: lex.rhymes(word, 60), related: lex.related(word), senses: lex.senses(word) });
    ctx.data_mut(|d| d.insert_temp(id, r.clone()));
    Some(r)
}

/// The colour of a rhyme kind (as the Lyrics tab's legend).
fn kind_color(k: RhymeKind) -> Color32 {
    use soundcraft_engine::lyric_tools::MarkKind;
    crate::lyrics_panel::mark_color(match k {
        RhymeKind::Perfect => MarkKind::Perfect,
        RhymeKind::Slant => MarkKind::Slant,
        RhymeKind::Multi => MarkKind::Multi,
    })
}

/// A suggestion button: the word, then its syllable count, dim. True when clicked.
fn word_button(ui: &mut Ui, word: &str, syllables: u32, t: &Tokens) -> bool {
    let mut job = LayoutJob::default();
    job.append(word, 0.0, TextFormat { font_id: regular(13.0), color: t.text, ..TextFormat::default() });
    job.append(&format!(" {syllables}"), 0.0, TextFormat { font_id: regular(10.0), color: t.text_dim.gamma_multiply(0.8), ..TextFormat::default() });
    ui.add(egui::Button::new(job).small()).on_hover_text(format!("{syllables} syllable{}", if syllables == 1 { "" } else { "s" })).clicked()
}

/// Keeps a capitalised word capitalised.
fn match_case(old: &str, new: &str) -> String {
    if old.chars().next().is_some_and(char::is_uppercase) {
        let mut c = new.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    } else {
        new.to_string()
    }
}

/// Uses a suggestion: replaces the target word in the sheet (one undo step), or copies it.
fn pick(app: &mut SoundApp, ctx: &egui::Context, f: &mut Finder, word: &str) {
    let Some(t) = f.target else {
        ctx.copy_text(word.to_string());
        app.ui.status = format!("Copied \u{2018}{word}\u{2019}");
        return;
    };
    let old = app.engine.session().lyric_sheet.lines.iter().find(|l| l.id == t.line).and_then(|l| l.text.get(t.start..t.end)).map(str::to_string);
    let Some(old) = old else {
        f.target = None;
        return;
    };
    let with = match_case(&old, word);
    app.engine.end_merge();
    match app.run("lyrics.replace_word", json!({"line": t.line, "start": t.start, "end": t.end, "with": with})) {
        Ok(_) => f.target = Some(Target { end: t.start + with.len(), ..t }),
        Err(e) => app.ui.status = e,
    }
}

pub(crate) fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut f = state(ctx);
    if !f.open {
        return;
    }
    let t = Tokens::current();
    let mut open = true;
    egui::Window::new("Word Finder")
        .id(egui::Id::new("word-finder-window"))
        .default_pos(egui::pos2(ctx.content_rect().right() - 760.0, ctx.content_rect().bottom() - 640.0))
        .open(&mut open)
        .default_size([400.0, 480.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut f.query).desired_width(160.0).hint_text("a word"));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) || ui.button("Look up").clicked() {
                    f.word = f.query.trim().to_string();
                    f.target = None;
                    f.stress = None;
                }
                let note = match f.target {
                    Some(_) => "click a word to put it in the line".to_string(),
                    None => "click a word to copy it".to_string(),
                };
                ui.label(egui::RichText::new(note).size(11.0).color(t.text_dim));
            });
            ui.horizontal(|ui| {
                for (tab, label) in [(Tab::Rhymes, "Rhymes"), (Tab::Synonyms, "Synonyms"), (Tab::Definition, "Definition")] {
                    ui.selectable_value(&mut f.tab, tab, label);
                }
            });
            ui.separator();
            if f.word.is_empty() {
                ui.label(egui::RichText::new("Type a word, or right-click a word of the lyrics \u{203A} Find Words").color(t.text_dim));
                return;
            }
            let Some(res) = results(ui.ctx(), &f.word) else {
                ui.label(egui::RichText::new("Loading the dictionary\u{2026}").color(t.text_dim).italics());
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
                return;
            };
            let syl = soundcraft_engine::lyric_text::syllables(&f.word);
            let stress = lexicon::stress_text(&soundcraft_engine::lyric_text::word_stresses(&f.word));
            ui.label(
                egui::RichText::new(format!("{}  \u{00B7}  {syl} syllable{}  \u{00B7}  {stress}", f.word, if syl == 1 { "" } else { "s" }))
                    .font(bold(14.0))
                    .color(t.text),
            );
            let mut chosen: Option<String> = None;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match f.tab {
                Tab::Rhymes => rhymes_tab(ui, &res, &mut f, &t, &mut chosen),
                Tab::Synonyms => synonyms_tab(ui, &res, &t, &mut chosen),
                Tab::Definition => definition_tab(ui, &res, &t, &mut chosen),
            });
            if let Some(w) = chosen {
                pick(app, ui.ctx(), &mut f, &w);
            }
        });
    f.open = open;
    ctx.data_mut(|d| d.insert_temp(key(), f));
}

fn rhymes_tab(ui: &mut Ui, res: &Results, f: &mut Finder, t: &Tokens, chosen: &mut Option<String>) {
    if res.rhymes.is_empty() {
        let known = lexicon::get().is_some_and(|l| l.syllables(&res.word).is_some());
        ui.label(egui::RichText::new(if known { "No rhymes found" } else { "Not in the pronouncing dictionary" }).color(t.text_dim));
        return;
    }
    let mut patterns: Vec<&str> = res.rhymes.iter().map(|r| r.stress.as_str()).collect();
    patterns.sort_by_key(|p| (p.matches('-').count(), *p));
    patterns.dedup();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("stress").color(t.text_dim));
        egui::ComboBox::from_id_salt("word-finder-stress").selected_text(f.stress.clone().unwrap_or_else(|| "any".into())).show_ui(ui, |ui| {
            ui.selectable_value(&mut f.stress, None, "any");
            for p in &patterns {
                ui.selectable_value(&mut f.stress, Some((*p).to_string()), *p);
            }
        });
    });
    for (kind, title) in [(RhymeKind::Perfect, "Perfect"), (RhymeKind::Multi, "Multi-syllable"), (RhymeKind::Slant, "Slant")] {
        let list: Vec<&Found> = res.rhymes.iter().filter(|r| r.kind == kind && f.stress.as_deref().is_none_or(|s| s == r.stress)).collect();
        if list.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, kind_color(kind));
            ui.label(egui::RichText::new(title).font(bold(13.0)).color(t.text));
        });
        let mut counts: Vec<u32> = list.iter().map(|r| r.syllables).collect();
        counts.sort_unstable();
        counts.dedup();
        for n in counts {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("{n} syl")).size(11.0).color(t.text_dim));
                for r in list.iter().filter(|r| r.syllables == n) {
                    if word_button(ui, &r.word, r.syllables, t) {
                        *chosen = Some(r.word.clone());
                    }
                }
            });
        }
    }
}

fn synonyms_tab(ui: &mut Ui, res: &Results, t: &Tokens, chosen: &mut Option<String>) {
    let r = &res.related;
    let groups =
        [("Synonyms", &r.synonyms), ("Similar", &r.similar), ("Broader", &r.broader), ("Narrower", &r.narrower), ("Opposites", &r.opposites)];
    if groups.iter().all(|(_, l)| l.is_empty()) {
        ui.label(egui::RichText::new("Not in WordNet").color(t.text_dim));
        return;
    }
    for (title, list) in groups {
        if list.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new(title).font(bold(13.0)).color(t.text));
        ui.horizontal_wrapped(|ui| {
            for w in list.iter().take(40) {
                if word_button(ui, w, soundcraft_engine::lyric_text::line_syllables(w), t) {
                    *chosen = Some(w.clone());
                }
            }
        });
    }
}

fn definition_tab(ui: &mut Ui, res: &Results, t: &Tokens, chosen: &mut Option<String>) {
    if res.senses.is_empty() {
        ui.label(egui::RichText::new("Not in WordNet").color(t.text_dim));
        return;
    }
    for pos in ["noun", "verb", "adjective", "adverb"] {
        let senses: Vec<&Sense> = res.senses.iter().filter(|s| s.pos == pos).collect();
        if senses.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new(pos).font(bold(13.0)).italics().color(t.text));
        for (i, s) in senses.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("{}.", i + 1)).color(t.text_dim));
                ui.label(egui::RichText::new(&s.definition).color(t.text));
            });
            for ex in s.examples.iter().take(2) {
                ui.label(egui::RichText::new(format!("    \u{201C}{ex}\u{201D}")).italics().size(12.0).color(t.text_dim));
            }
            if !s.synonyms.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(14.0);
                    for w in s.synonyms.iter().take(8) {
                        if word_button(ui, w, soundcraft_engine::lyric_text::line_syllables(w), t) {
                            *chosen = Some(w.clone());
                        }
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_is_kept() {
        assert_eq!(match_case("Strong", "tall"), "Tall");
        assert_eq!(match_case("strong", "tall"), "tall");
    }
}
