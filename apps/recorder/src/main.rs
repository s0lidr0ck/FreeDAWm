//! `freedawm-recorder`: FreeDAWm Recorder's window (see the library's docs and `docs/recorder.md`).
// A GUI program on Windows: no console window opens with it.
#![windows_subsystem = "windows"]

use soundcraft_recorder::settings::{Paths, Prefs};
use soundcraft_recorder::ui::{RecorderApp, look};

struct App(RecorderApp);

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(look::BG)).show(ui, |ui| self.0.ui(ui));
    }

    fn on_exit(&mut self) {
        self.0.shutdown();
    }
}

/// The window's size and place as last left: the design's 480×640 the first time; a size
/// smaller than the least it can be, or a place far off any screen, is not used.
fn window(prefs: &Prefs) -> egui::ViewportBuilder {
    let size =
        prefs.window_size.filter(|s| s[0] >= MIN[0] && s[1] >= MIN[1] && s[0] <= 4000.0 && s[1] <= 3000.0).unwrap_or([look::WIDTH, look::HEIGHT]);
    let v = egui::ViewportBuilder::default()
        .with_title("FreeDAWm Recorder")
        .with_app_id("io.github.s0lidr0ck.FreeDAWm.Recorder")
        .with_inner_size(size)
        .with_min_inner_size(MIN);
    match prefs.window_pos.filter(|p| p[0].is_finite() && p[1].is_finite() && (-2000.0..8000.0).contains(&p[0]) && (-200.0..5000.0).contains(&p[1])) {
        Some(p) => v.with_position(p),
        None => v,
    }
}

/// The least the window can be: the column's buttons still fit, the rest scrolls.
const MIN: [f32; 2] = [380.0, 520.0];

fn main() -> eframe::Result {
    let paths = Paths::system();
    let prefs = paths.as_ref().map(|p| Prefs::load(&p.prefs)).unwrap_or_default();
    let options = eframe::NativeOptions { viewport: window(&prefs), centered: prefs.window_pos.is_none(), ..Default::default() };
    eframe::run_native(
        "FreeDAWm Recorder",
        options,
        Box::new(move |cc| {
            look::apply(&cc.egui_ctx);
            // No device opens here: the window paints first, the devices open on its second frame.
            Ok(Box::new(App(RecorderApp::open(paths))))
        }),
    )
}
