//! "Open in FreeDAWm": the Recorder starts FreeDAWm with the take's WAV and MIDI file; FreeDAWm
//! imports the files named on its command line into the new, untitled session it opens, each at
//! the insertion point (the start), as `apps/soundcraft`'s `open_path` does. That import is run
//! here on a new engine with a real take's files.

mod common;

use common::{offline, ramp, temp_dir};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::TrackKind;
use soundcraft_recorder::rig::InputChoice;
use soundcraft_recorder::ui::{daw_args, daw_beside};

#[test]
fn open_in_freedawm_names_the_take_and_freedawm_puts_it_at_the_start_of_a_new_session() {
    let dir = temp_dir("daw");
    let mut o = offline(1, &["Keys"], InputChoice { on: true, channel: 0, stereo: false });
    o.rig.start(&dir, "Idea 2026-11-23 14-03-10", false, 0).unwrap();
    o.run(0.3, &ramp);
    o.keys.send("Keys", &[0x90, 60, 100]);
    o.run(0.2, &ramp);
    o.keys.send("Keys", &[0x80, 60, 0]);
    o.run(0.2, &ramp);
    o.rig.stop();
    let take = soundcraft_recorder::files::recent(&dir, 10).remove(0);
    let args = daw_args(&take);
    assert_eq!(args.len(), 2);
    assert!(
        args[0].to_string_lossy().ends_with("Idea 2026-11-23 14-03-10.wav") && args[1].to_string_lossy().ends_with("Idea 2026-11-23 14-03-10.mid")
    );

    // FreeDAWm beside the Recorder, or not.
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join(if cfg!(windows) { "freedawm-recorder.exe" } else { "freedawm-recorder" });
    assert_eq!(daw_beside(&exe), None, "not there: the button says so");
    let daw = bin.join(if cfg!(windows) { "freedawm.exe" } else { "freedawm" });
    std::fs::write(&daw, b"").unwrap();
    assert_eq!(daw_beside(&exe), Some(daw));

    // What FreeDAWm does with them, in a new session.
    let mut e = Engine::default();
    for a in &args {
        let p = a.to_string_lossy().into_owned();
        let at = e.session().edit.selection.start;
        let id = if p.ends_with(".mid") { "file.import_midi" } else { "file.import_audio" };
        e.execute(id, &json!({"path": p, "at": at})).unwrap();
    }
    let s = e.session();
    assert!(e.path.is_none(), "untitled");
    assert_eq!(s.tracks.len(), 2, "{:?}", s.tracks.iter().map(|t| (&t.name, t.kind)).collect::<Vec<_>>());
    let audio = s.tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap();
    assert_eq!(audio.clips().len(), 1);
    assert_eq!(audio.clips()[0].start, 0, "the recording at the start");
    let keys = s.tracks.iter().find(|t| t.kind == TrackKind::Instrument).unwrap();
    let clip = keys.clips().iter().find(|c| c.midi_playing().is_some()).unwrap();
    assert_eq!(clip.start, 0, "the notes' clip at the start, the notes where they were played");
    assert_eq!(clip.midi_playing().unwrap().notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60]);
    let _ = std::fs::remove_dir_all(dir);
}
