//! More Mix windows (Window › Separate Windows › New Mix Window): each in its own native window, with its own find
//! filter and spill ([`crate::mix_find`] keeps one per window) and its own shown sections (its
//! Sections menu), as Pro Tools' several Mix windows and Cubase's MixConsoles 1 to 4 have. The
//! first Mix window (in the main window or Mix in Separate Window) keeps View › Mix Window Views.
//! They are views only: nothing is saved in the session or undone, and they close with the app.

use crate::SoundApp;
use crate::theme::Tokens;
use serde_json::{Value, json};

/// The most Mix windows besides the first.
pub const MAX_MORE: usize = 3;

/// One more Mix window: its number (2 and up, shown in its title) and the sections it shows.
#[derive(Debug, Clone, PartialEq)]
pub struct MoreMix {
    pub n: u32,
    pub views: Vec<String>,
}

/// The sections a Mix window can show, as View › Mix Window Views names them: (id, label).
pub const SECTIONS: [(&str, &str); 8] = [
    ("inserts_ae", "Inserts A-E"),
    ("inserts_fj", "Inserts F-J"),
    ("sends_ae", "Sends A-E"),
    ("sends_fj", "Sends F-J"),
    ("io", "I/O"),
    ("pitch", "Pitch"),
    ("color", "Track Color"),
    ("comments", "Comments"),
];

/// The viewport of Mix window `n`.
pub fn viewport(n: u32) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("soundcraft.mix", n))
}

/// Whether `vp` is one of the more Mix windows (their find and spill are their own).
pub fn is_more(app_more: &[MoreMix], vp: egui::ViewportId) -> bool {
    app_more.iter().any(|m| viewport(m.n) == vp)
}

/// `window.new_mix`: opens one more Mix window showing what the first shows (refused past
/// [`MAX_MORE`]).
pub fn open(app: &mut SoundApp) -> Result<Value, String> {
    if app.ui.more_mix.len() >= MAX_MORE {
        return Err(format!("At most {} Mix windows", MAX_MORE + 1));
    }
    let n = (2..).find(|n| !app.ui.more_mix.iter().any(|m| m.n == *n)).unwrap_or(2);
    app.ui.more_mix.push(MoreMix { n, views: app.ui.mix_views.clone() });
    Ok(json!({"window": n}))
}

/// Shows or hides `section` in Mix window `n` (only that window).
pub fn toggle_section(app: &mut SoundApp, n: u32, section: &str) {
    if let Some(m) = app.ui.more_mix.iter_mut().find(|m| m.n == n) {
        if m.views.iter().any(|v| v == section) {
            m.views.retain(|v| v != section);
        } else {
            m.views.push(section.to_string());
        }
    }
}

/// Draws every more Mix window (each its own native window; a backend without them floats it).
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    for m in app.ui.more_mix.clone() {
        let title = format!("{}  —  Mix {}", app.engine.session().name, m.n);
        let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([1000.0, 680.0]).with_min_inner_size([480.0, 320.0]);
        let mut closed = false;
        ctx.show_viewport_immediate(viewport(m.n), builder, |ui, _class| {
            ui.set_style(ui.ctx().global_style());
            let own = ui.ctx().viewport_id() != egui::ViewportId::ROOT;
            if own {
                crate::shortcuts::handle(app, ui.ctx());
            }
            egui::Panel::top(egui::Id::new(("mix_more_bar", m.n))).frame(egui::Frame::NONE.fill(Tokens::current().toolbar_bg)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Mix {}", m.n));
                    ui.menu_button("Sections ▾", |ui| {
                        for (id, label) in SECTIONS {
                            if ui.add(egui::Button::new(label).selected(m.views.iter().any(|v| v == id))).clicked() {
                                toggle_section(app, m.n, id);
                            }
                        }
                    });
                });
            });
            // This window's sections while it draws; the first window's are put back after.
            let views = app.ui.more_mix.iter().find(|x| x.n == m.n).map(|x| x.views.clone()).unwrap_or_default();
            let first = std::mem::replace(&mut app.ui.mix_views, views);
            // Its widgets' state (scroll positions…) is its own.
            ui.push_id(("mix_more", m.n), |ui| crate::mix_window::show(app, ui));
            app.ui.mix_views = first;
            closed = ui.ctx().input(|i| i.viewport().close_requested());
        });
        if closed {
            app.ui.more_mix.retain(|x| x.n != m.n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each more Mix window has its own sections: toggling one leaves the first window's and
    /// the others' alone; a fourth more window is refused.
    #[test]
    fn more_mix_windows_keep_their_own_sections() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, crate::Services::default());
        app.ui.mix_views = vec!["io".into()];
        assert_eq!(app.run("window.new_mix", json!({})).unwrap()["window"], 2);
        assert_eq!(app.run("window.new_mix", json!({})).unwrap()["window"], 3);
        toggle_section(&mut app, 2, "comments");
        toggle_section(&mut app, 3, "io");
        assert_eq!(app.ui.more_mix[0].views, ["io", "comments"]);
        assert!(app.ui.more_mix[1].views.is_empty());
        assert_eq!(app.ui.mix_views, ["io"], "the first window's sections are its own");
        app.run("window.new_mix", json!({})).unwrap();
        assert!(app.run("window.new_mix", json!({})).is_err(), "at most four");
        assert_ne!(viewport(2), viewport(3));
        assert!(is_more(&app.ui.more_mix, viewport(3)) && !is_more(&app.ui.more_mix, crate::window_sync::mix_viewport()));
    }
}
