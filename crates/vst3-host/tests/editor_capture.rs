//! `captures_a_real_vst3_editor`, ignored by default (it opens a window on screen): run with
//! `cargo test -p soundcraft-vst3-host --test editor_capture -- --ignored`. `harness = false`, as
//! editors open on the process main thread only. Opens one installed plugin's editor
//! (`SOUNDCRAFT_CAPTURE_PLUGIN`, a `.vst3` bundle path; default AG12 from the system folder),
//! lets it draw for 1.5 s, captures it and writes the picture to
//! `CARGO_TARGET_TMPDIR/editor_capture.png` for a look. The plugin is copied to a folder of its
//! own, which is the first folder the host learns about, so it never scans every installed plugin.

use std::path::{Path, PathBuf};
use std::time::Duration;

fn captures_a_real_vst3_editor(bundle: &Path) {
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("editor-capture");
    let _ = std::fs::remove_dir_all(&folder);
    copy(bundle, &folder.join(bundle.file_name().expect("a bundle name"))).expect("copy the plugin");
    assert!(soundcraft_vst3_host::add_search_dir(&folder) >= 1, "the plugin is found");
    let descs = soundcraft_vst3_host::cached().expect("the folder's plugins");
    let d = descs.first().expect("a plugin class");
    let mut p = soundcraft_vst3_host::instantiate_plugin(&d.id).expect("instantiate");
    assert!(p.capture_editor().is_none(), "closed: nothing to capture");
    soundcraft_dsp::Plugin::open_editor(&mut p).expect("open the editor");
    soundcraft_vst3_host::pump_editor_messages(Duration::from_millis(1500));
    let shot = p.capture_editor();
    soundcraft_dsp::Plugin::close_editor(&mut p);
    soundcraft_vst3_host::pump_editor_messages(Duration::from_millis(100));
    assert!(p.capture_editor().is_none(), "closed again");
    let (w, h, rgba) = shot.expect("a picture of the open editor");
    assert_eq!(rgba.len(), w as usize * h as usize * 4);
    assert!(w >= 100 && h >= 60, "{w}x{h}");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("editor_capture.png");
    image::RgbaImage::from_raw(w, h, rgba).expect("whole rows").save(&out).expect("write the picture");
    println!("captures_a_real_vst3_editor: {} ({}) {w}x{h} -> {}", d.name, d.vendor, out.display());
    drop(p);
    soundcraft_vst3_host::shutdown();
}

/// Copies a file or a bundle folder.
fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

fn main() {
    let ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    if !ignored || !cfg!(windows) {
        println!("captures_a_real_vst3_editor: ignored (run with -- --ignored, on Windows)");
        return;
    }
    let bundle = std::env::var_os("FREEDAWM_CAPTURE_PLUGIN")
        .or_else(|| std::env::var_os("SOUNDCRAFT_CAPTURE_PLUGIN"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Common Files\VST3\AG12.vst3"));
    if !bundle.exists() {
        println!("captures_a_real_vst3_editor: SKIPPED ({} is not installed)", bundle.display());
        return;
    }
    captures_a_real_vst3_editor(&bundle);
    println!("editor_capture: ok");
}
