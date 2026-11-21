//! The status bar shows blocks of audio played from disk that came too late ("Disk too slow"),
//! next to the device's underruns, and Reset Underruns (a click on it) clears both.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use egui_kittest::kittest::Queryable;
use soundcraft_model::SourceAudio;
use soundcraft_model::disk::{self, BLOCK, DiskFile};
use soundcraft_ui_egui::{Services, SoundApp};

fn step(h: &mut egui_kittest::Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

/// A block of a file played from disk that the audio thread asks for before it is read.
fn miss_a_block() {
    let d = std::env::temp_dir().join(format!("freedawm-status-disk-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; BLOCK * 3]] };
    let opts = soundcraft_audio_io::EncodeOptions { format: soundcraft_audio_io::FileFormat::Wav, ..Default::default() };
    let p = d.join("late.wav");
    std::fs::write(&p, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
    let audio = SourceAudio::from_disk(DiskFile::open(&p).unwrap(), SourceAudio::new(buf).peaks).unwrap();
    let before = disk::underruns();
    assert_eq!(audio.cursor(0, true).get(BLOCK * 2 + 1), Some(0.0), "not read yet: silence");
    assert!(disk::underruns() > before);
}

#[test]
fn the_status_bar_counts_late_disk_blocks_and_reset_clears_them() {
    let app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    disk::reset_underruns();
    for _ in 0..3 {
        step(&mut h);
    }
    assert!(h.query_by_label_contains("Disk too slow").is_none(), "nothing late: nothing shown");
    miss_a_block();
    for _ in 0..3 {
        step(&mut h);
    }
    let late = disk::underruns_since_reset();
    assert!(late > 0);
    assert!(h.query_by_label(&format!("Disk too slow: {late}")).is_some(), "shown in the status bar with the count");
    h.query_by_label_contains("Disk too slow").unwrap().click();
    for _ in 0..3 {
        step(&mut h);
    }
    assert_eq!(disk::underruns_since_reset(), 0, "reset");
    assert!(disk::underruns() > 0, "the total since start-up keeps counting");
    assert!(h.query_by_label_contains("Disk too slow").is_none(), "cleared");
}
