//! The DAWproject import report (spec `docs/superpowers/specs/2026-10-31-dawproject-import-export-design.md`):
//! after File › Import › DAWproject…, a window lists what came across and what didn't, by track:
//! placeholders (plugins not installed, other DAWs' built-in devices), what changed on the way and
//! what was not carried. The same text is in the Notes tab.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use egui::{RichText, vec2};
use soundcraft_engine::dawproject::{Fate, Report};

/// Shows the report from an import's result (`file.import_dawproject`), if it has one.
pub fn offer(app: &mut SoundApp, result: &serde_json::Value) {
    if let Some(r) = result.get("report").and_then(|r| serde_json::from_value::<Report>(r.clone()).ok()) {
        app.ui.status = r.headline();
        app.dawproject_report = Some(r);
    }
}

fn fate_color(f: Fate, t: &Tokens) -> egui::Color32 {
    match f {
        Fate::Placeholder => t.accent,
        Fate::Changed => egui::Color32::from_rgb(214, 170, 70),
        Fate::Dropped => egui::Color32::from_rgb(214, 96, 86),
    }
}

/// The window, while there is a report to show.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(report) = &app.dawproject_report else { return };
    let t = Tokens::current();
    let mut open = true;
    let mut close = false;
    egui::Window::new(RichText::new("DAWproject Import").font(bold(13.0)))
        .id(egui::Id::new("dawproject-report"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(vec2(560.0, 420.0))
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -40.0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            ui.label(RichText::new(&report.file).font(bold(14.0)));
            if !report.application.is_empty() {
                ui.label(RichText::new(format!("Written by {}", report.application)).color(t.text_dim));
            }
            ui.separator();
            ui.label(RichText::new("Came across").font(bold(12.0)));
            ui.label(report.carried.join(" · "));
            if !report.plugins.is_empty() {
                ui.label(RichText::new(format!("Plugins with their state: {}", report.plugins.join(", "))).color(t.text_dim));
            }
            ui.add_space(4.0);
            let counts: Vec<String> = [Fate::Placeholder, Fate::Changed, Fate::Dropped]
                .into_iter()
                .filter_map(|f| {
                    let n = report.count(f);
                    (n > 0).then(|| f.counted(n))
                })
                .collect();
            if counts.is_empty() {
                ui.label(RichText::new("Everything came across.").color(t.text_dim));
            } else {
                ui.label(RichText::new(format!("Not carried exactly · {}", counts.join(" · "))).font(bold(12.0)));
                egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("dawproject-report-items").num_columns(3).striped(true).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                        for fate in [Fate::Placeholder, Fate::Changed, Fate::Dropped] {
                            for i in report.items.iter().filter(|i| i.fate == fate) {
                                ui.label(RichText::new(fate.label()).color(fate_color(fate, &t)).font(bold(11.0)));
                                ui.label(if i.track.is_empty() { RichText::new("Project").color(t.text_dim) } else { RichText::new(&i.track) });
                                ui.label(format!("{} — {}", i.what, i.why));
                                ui.end_row();
                            }
                        }
                    });
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Saved in the Notes tab (#dawproject).").color(t.text_dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("OK").strong()).fill(t.accent)).clicked() {
                        close = true;
                    }
                });
            });
        });
    if !open || close {
        app.dawproject_report = None;
    }
}
