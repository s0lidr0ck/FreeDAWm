//! `freedawm-recorder`: FreeDAWm Recorder's window (see the library's docs and `docs/recorder.md`).
// A GUI program on Windows: no console window opens with it.
#![windows_subsystem = "windows"]

use soundcraft_recorder::settings::Paths;
use soundcraft_recorder::ui::{RecorderApp, look};

struct App(RecorderApp);

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| self.0.ui(ui));
    }

    fn on_exit(&mut self) {
        self.0.shutdown();
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FreeDAWm Recorder")
            .with_app_id("io.github.s0lidr0ck.FreeDAWm.Recorder")
            .with_inner_size([520.0, 720.0])
            .with_min_inner_size([420.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "FreeDAWm Recorder",
        options,
        Box::new(|cc| {
            look::apply(&cc.egui_ctx);
            Ok(Box::new(App(RecorderApp::open(Paths::system()))))
        }),
    )
}
