//! The Vocal Editor in its own native window (Window › Vocal Editor in Separate Window, or the
//! title row's Window button): a second egui viewport, as the separate Mix window. It is
//! resizable, its position and size are remembered in `ui.json`, and closing it docks the editor
//! back into the Edit window. Backends without multiple windows show it as a floating egui window.

use crate::SoundApp;
use egui::ViewportId;

/// The separate Vocal Editor window's viewport.
pub fn viewport() -> ViewportId {
    ViewportId::from_hash_of("soundcraft.vocal")
}

/// The viewport the editor was last drawn in (its keys are read there): its own window's, or
/// the main one (docked, or a backend that embeds viewports).
pub fn home(ctx: &egui::Context) -> ViewportId {
    ctx.data(|d| d.get_temp::<ViewportId>(egui::Id::new("soundcraft-vocal-viewport"))).unwrap_or(ViewportId::ROOT)
}

/// The default inner size of the window, before it was ever moved or resized.
pub const DEFAULT_SIZE: [f32; 2] = [1200.0, 720.0];

/// Draws the editor in its own window.
pub fn show_separate(app: &mut SoundApp, ctx: &egui::Context) {
    let title = format!("{}  \u{2014}  Vocal Editor", app.engine.session().name);
    let mut builder = egui::ViewportBuilder::default().with_title(title).with_min_inner_size([520.0, 320.0]).with_resizable(true);
    match app.ui.vocal_window.filter(|r| r.iter().all(|v| v.is_finite()) && r[2] >= 100.0 && r[3] >= 100.0) {
        Some([x, y, w, h]) => builder = builder.with_position([x, y]).with_inner_size([w, h]),
        None => builder = builder.with_inner_size(DEFAULT_SIZE),
    }
    ctx.show_viewport_immediate(viewport(), builder, |ui, _class| {
        ui.set_style(ui.ctx().global_style());
        if std::mem::take(&mut app.focus_vocal) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        let own = ui.ctx().viewport_id() != ViewportId::ROOT;
        if own {
            // Its keys: transport, undo, save… work from here too (Space and the editor's own
            // keys are left to it, see `owns_key`).
            crate::shortcuts::handle(app, ui.ctx());
            // Remember where it is and how big.
            let info = ui.ctx().input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
            if let (Some(outer), Some(inner)) = info {
                app.ui.vocal_window = Some([outer.min.x, outer.min.y, inner.width(), inner.height()]);
            }
        }
        super::show(app, ui);
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            separate_closed(app);
        }
    });
}

/// The separate window was closed: the editor docks back into the Edit window.
pub fn separate_closed(app: &mut SoundApp) {
    app.ui.vocal_separate = false;
    app.ui.show_vocal_editor = true;
}
