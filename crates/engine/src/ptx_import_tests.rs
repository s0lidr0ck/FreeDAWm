//! File › Import › Pro Tools Session... on the two clean-room sessions of ptx-session-kit
//! (`crates/ptx/tests/fixtures`), checked against what their build specifications say.

use super::*;
use soundcraft_model::{ClipContent, Track};
use stock::cleanq::clean_q_dsp as cq;

fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ptx/tests/fixtures").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn track<'a>(s: &'a Session, name: &str) -> &'a Track {
    s.tracks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no track {name}: {:?}", s.tracks.iter().map(|t| &t.name).collect::<Vec<_>>()))
}

fn lane(t: &Track, p: &AutoParam) -> Vec<(Samples, f32)> {
    t.automation.iter().find(|l| &l.param == p).map(|l| l.points.iter().map(|x| (x.at, x.value)).collect()).unwrap_or_default()
}

fn param(t: &Track, plugin: &str, key: &str) -> f32 {
    let m = t.mixer.inserts.iter().find(|m| m.plugin == plugin).unwrap_or_else(|| panic!("{}: no {plugin}", t.name));
    *m.params.get(key).unwrap_or_else(|| panic!("{}: {plugin} has no {key}: {:?}", t.name, m.params))
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn notes(c: &Clip) -> Vec<(i64, u8, i64, u8)> {
    match &c.content {
        ClipContent::Midi { sequence } => sequence.notes.iter().map(|n| (n.start, n.pitch, n.length, n.velocity)).collect(),
        _ => Vec::new(),
    }
}

fn seq(c: &Clip) -> &Sequence {
    match &c.content {
        ClipContent::Midi { sequence } => sequence,
        _ => panic!("{} is not MIDI", c.name),
    }
}

#[test]
fn cr_min_comes_in_as_its_specification_says_in_one_undo_step() {
    let mut e = Engine::default();
    let steps = e.undo_history().len();
    let dir = std::env::temp_dir().join(format!("freedawm-ptx-min-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("CR_min.ptx");
    std::fs::write(&path, fixture("CR_min.ptx")).unwrap();
    let v = e.execute("file.import_ptx", &json!({"path": path.to_string_lossy()})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(e.undo_history().len(), steps + 1, "one undo step");
    let s = e.session();
    // The session's timeline: 25 fps from 01:00:00:00, 120 BPM 4/4, C major then A minor at bar 2.
    assert_eq!(s.frame_rate, FrameRate::Fps25);
    assert_eq!(s.timecode_start, 3600 * 48_000);
    assert_eq!(s.tempo.tempos().len(), 1);
    assert!((s.tempo.tempos()[0].bpm - 120.0).abs() < 1e-9);
    assert_eq!((s.tempo.meters()[0].numerator, s.tempo.meters()[0].denominator), (4, 4));
    assert_eq!(s.key_signatures, vec![(0, "C major".to_string()), (96_000, "A minor".to_string())]);
    // Tracks in order, with their kinds, widths, comments and colour.
    let names: Vec<&str> = s.tracks.iter().map(|t| t.name.as_str()).collect();
    let au = names.iter().position(|n| *n == "Au_Lead").unwrap();
    let keys = names.iter().position(|n| *n == "In_Keys").unwrap();
    let bass = names.iter().position(|n| *n == "Md_Bass").unwrap();
    assert!(au < keys && keys < bass, "{names:?}");
    let lead = track(s, "Au_Lead");
    assert_eq!((lead.kind, lead.format), (TrackKind::Audio, ChannelFormat::Mono));
    assert_eq!(lead.comments, "Clean-room mono audio track");
    assert_eq!(Some(lead.color), pt_colour(29).map(|c| c.1));
    assert_eq!((track(s, "In_Keys").kind, track(s, "In_Keys").format), (TrackKind::Instrument, ChannelFormat::Stereo));
    assert_eq!(track(s, "Md_Bass").kind, TrackKind::Midi);
    // The audio clip, its fades (equal power), offline: there is no Audio Files folder here.
    let c = &lead.clips()[0];
    assert_eq!((c.name.as_str(), c.start, c.length), ("lead_m", 0, 192_000));
    assert_eq!((c.fade_in.len, c.fade_in.shape), (4_800, FadeShape::EqualPower));
    assert_eq!((c.fade_out.len, c.fade_out.shape), (9_600, FadeShape::EqualPower));
    assert_eq!(v["offline"].as_array().unwrap().len(), 1, "{v}");
    // Volume in tenths of a dB, pan in hundredths: 0, -3, 0 dB and -0.4, 0.4, 0.
    assert_eq!(lane(lead, &AutoParam::Volume), vec![(0, 0.0), (96_000, -3.0), (192_000, 0.0)]);
    let p = lane(lead, &AutoParam::Pan(0));
    assert!(p.iter().zip([(0, -0.4), (96_000, 0.4), (192_000, 0.0)]).all(|(a, b)| a.0 == b.0 && close(a.1, b.1, 1e-6)), "{p:?}");
    assert!(close(lead.mixer.pan[0], -0.4, 1e-6));
    // The instrument's fader is at -2 dB; its lane is tick-based (2400 samples = 96 000 PT ticks).
    let k = track(s, "In_Keys");
    assert!(close(k.mixer.volume_db, -2.0, 1e-6));
    assert_eq!(lane(k, &AutoParam::Volume), vec![(0, -2.0), (2_400, -2.0), (4_800, -4.5)]);
    // MIDI: the chords C, Am, F, G7 a bar each at velocity 96; the bass line 36, 33, 29, 31.
    let kc = &k.clips()[0];
    assert_eq!((kc.start, kc.length), (0, 384_000));
    let n = notes(kc);
    assert_eq!(n.len(), 13);
    assert_eq!(&n[..3], &[(0, 60, 3840, 96), (0, 64, 3840, 96), (0, 67, 3840, 96)]);
    let b = notes(&track(s, "Md_Bass").clips()[0]);
    assert_eq!(b.iter().map(|x| (x.0, x.1, x.3)).collect::<Vec<_>>(), vec![(0, 36, 100), (3840, 33, 95), (7680, 29, 95), (11_520, 31, 100)]);
    // Markers: a point at 0 and a selection over bar 2.
    let intro = s.markers.iter().find(|m| m.name == "Intro").unwrap();
    assert_eq!((intro.kind, intro.start), (MarkerKind::Marker, 0));
    let chorus = s.markers.iter().find(|m| m.name == "Chorus").unwrap();
    assert_eq!((chorus.kind, chorus.start, chorus.end), (MarkerKind::Selection, 96_000, 192_000));
    // Plug-ins: EQ3 7-Band's mid band +4.5 dB at 1 kHz as CleanQ, Dyn3 as the compressor, the
    // master's Trim at -1.5 dB (the reader's curve: within 0.1 dB).
    assert!(close(param(lead, "fs_cleanq", "b04_gain"), cq::gain_to_knob(4.5), 1e-6));
    assert!(close(param(lead, "fs_cleanq", "b04_freq"), cq::freq_to_knob(1000.0), 1e-6));
    assert_eq!(param(lead, "fs_cleanq", "b04_on"), 2.0);
    assert_eq!(param(lead, "fs_cleanq", "b04_type"), cq::TYPE_BELL as f32);
    assert_eq!(param(lead, "fs_cleanq", "b01_on"), 0.0, "the HPF is out");
    assert_eq!((param(k, "compressor", "threshold"), param(k, "compressor", "ratio"), param(k, "compressor", "makeup")), (-18.0, 4.0, 2.5));
    let master = s.tracks.iter().find(|t| t.kind == TrackKind::Master).unwrap();
    assert!(close(param(master, "trim", "gain"), -1.5, 0.1), "{}", param(master, "trim", "gain"));
    assert_eq!(lane(master, &AutoParam::Volume), vec![(0, 0.0), (144_000, 0.0), (192_000, -6.0)]);
    // The report: in the Notes tab and the result.
    let text = v["report_text"].as_str().unwrap();
    assert!(v["summary"].as_str().unwrap().starts_with("Pro Tools CR_min.ptx: 4 tracks, 3 clips, 2 markers"), "{v}");
    assert!(text.contains("Offline: lead_m"), "{text}");
    assert!(text.contains("Approximated: \"In_Keys\": Dyn3 Compressor/Limiter (insert a) came in as Compressor/Limiter"), "{text}");
    assert!(text.contains("Not carried: \"Au_Lead\": the automation of EQ3 7-Band"), "{text}");
    assert!(s.notes.iter().any(|n| n.tags.contains(&"ptx".to_string()) && n.text.starts_with("Pro Tools CR_min.ptx")));
    // Undo takes all of it away.
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(e.session().tracks.iter().all(|t| t.name != "Au_Lead"));
}

#[test]
fn audio_in_the_sessions_audio_files_folder_is_found_and_referred_to() {
    let dir = std::env::temp_dir().join(format!("freedawm-ptx-audio-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Audio Files")).unwrap();
    // A short stand-in for the kit's lead_m.wav (made here, not copied): 0.5 s of a ramp.
    let pcm: Vec<u8> = (0..24_000i32).flat_map(|i| (((i % 200) - 100) as i16 * 50).to_le_bytes()).collect();
    std::fs::write(dir.join("Audio Files").join("lead_m.wav"), soundcraft_aaf::walk::pcm_wav(&pcm, 1, 48_000, 16)).unwrap();
    let mut e = Engine::default();
    let v = import_bytes(&mut e, &fixture("CR_min.ptx"), "CR_min.ptx", Some(&dir), None).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(v["offline"].as_array().unwrap().is_empty(), "{v}");
    let s = e.session();
    let src = track(s, "Au_Lead").clips()[0].source().unwrap();
    assert!(s.pool.get(src).is_some(), "the audio is in the session");
    assert!(s.source(src).unwrap().path.ends_with("lead_m.wav"));
}

#[test]
fn cr_max3_tracks_routing_groups_clips_and_midi() {
    let mut e = Engine::default();
    let v = import_bytes(&mut e, &fixture("CR_max3.ptx"), "CR_max3.ptx", None, None).unwrap();
    let s = e.session();
    assert_eq!(v["tracks"].as_array().unwrap().len(), 30, "29 tracks and the master: {v}");
    // Kinds and folders.
    assert_eq!(track(s, "Fold_Rhythm").kind, TrackKind::Folder);
    assert_eq!(track(s, "Fold_Strings").kind, TrackKind::Folder);
    assert_eq!(track(s, "Aux_Verb_St").kind, TrackKind::Aux);
    let drums = track(s, "Au_Drums_St");
    assert_eq!(drums.folder, Some(track(s, "Fold_Rhythm").id));
    assert_eq!(track(s, "Au_Gtr_St").folder, Some(track(s, "Fold_Strings").id));
    assert_eq!(track(s, "In_Lead_St").folder, Some(track(s, "Fold_Synths").id));
    // States: hidden, inactive, solo.
    assert!(track(s, "Au_Amb_M").hidden);
    assert!(track(s, "Md_Hidden").hidden);
    assert!(track(s, "In_Drone_St").inactive);
    assert!(track(s, "Au_Perc_St").mixer.solo);
    // Routing: the drums to Bus_Rhythm, which the routing folder takes; sends to the FX bus.
    let bus = |n: &str| s.bus_by_name(n).unwrap_or_else(|| panic!("no bus {n}")).id;
    assert_eq!(drums.mixer.output, Route::Bus(bus("Bus_Rhythm")));
    assert_eq!(track(s, "Fold_Rhythm").mixer.input, Route::Bus(bus("Bus_Rhythm")));
    assert_eq!(track(s, "Aux_Verb_St").mixer.input, Route::Bus(bus("Bus_FX")));
    let sends: Vec<&SendSlot> = drums.mixer.sends.iter().flatten().collect();
    assert_eq!(sends.len(), 3);
    assert_eq!(sends[0].target, Route::Bus(bus("Bus_FX")));
    assert!(close(sends[0].level_db, -6.0, 1e-6) && sends[0].pre_fader);
    assert!(close(sends[0].pan, -0.3, 1e-6));
    assert_eq!(sends[1].target, Route::Bus(bus("Bus_Par")));
    assert!(close(sends[1].level_db, -3.5, 1e-6) && !sends[1].pre_fader);
    // Stereo pans and their automation.
    assert!(close(drums.mixer.pan[0], -0.5, 1e-6) && close(drums.mixer.pan[1], 0.5, 1e-6));
    assert_eq!(lane(track(s, "Au_Bass_M"), &AutoParam::Mute), vec![(0, 0.0), (144_000, 1.0), (192_000, 0.0)], "mute automation");
    assert!(close(track(s, "Au_Bass_M").mixer.volume_db, -2.0, 1e-6));
    // Groups: edit+mix, edit only, mix only; <ALL> is not a group here.
    let g = |n: &str| s.groups.iter().find(|g| g.name == n).unwrap_or_else(|| panic!("no group {n}"));
    assert_eq!((g("Grp_Rhythm").edit, g("Grp_Rhythm").mix, g("Grp_Rhythm").members.len()), (true, true, 3));
    assert_eq!((g("Grp_Synths").edit, g("Grp_Synths").mix, g("Grp_Synths").active), (true, false, false));
    assert_eq!((g("Grp_AuxMix").edit, g("Grp_AuxMix").mix), (false, true));
    assert!(s.groups.iter().all(|g| g.name != "<ALL>"));
    // Clips: a sync point; a muted clip with its fades; a loop as copies; a clip-gain envelope.
    let b = &track(s, "Au_Bass_M").clips()[0];
    assert_eq!((b.start, b.sync_point), (0, 10_000));
    let fx = &track(s, "Au_FX_M").clips()[0];
    assert!(fx.muted);
    assert_eq!((fx.start, fx.length, fx.fade_in.len, fx.fade_out.len), (48_000, 144_000, 4_800, 4_800));
    assert_eq!(fx.fade_out.shape, FadeShape::Linear, "equal gain");
    let gtr = track(s, "Au_Gtr_St").clips();
    assert_eq!(gtr.len(), 4, "four loop copies");
    assert_eq!(gtr.iter().map(|c| c.start).collect::<Vec<_>>(), vec![0, 72_000, 144_000, 216_000]);
    assert!(gtr.iter().all(|c| c.loop_id.is_some() && c.loop_id == gtr[0].loop_id));
    let vox = &track(s, "Au_Vox_Lead_M").clips()[0];
    assert_eq!(vox.gain_env, vec![(0, -3.5), (48_000, 1.5), (144_000, -6.0), (240_000, 0.0)]);
    // The perc clip's fade out at its end (bar 2 in ticks).
    let perc = &track(s, "Au_Perc_St").clips()[0];
    assert_eq!((perc.length, perc.fade_out.len), (96_000, 4_800));
    // The ambience clip group, opened out and grouped.
    let amb = track(s, "Au_Amb_M").clips();
    assert!(!amb.is_empty() && amb.iter().all(|c| c.group.is_some() && c.group == amb[0].group), "{amb:?}");
    // Alternate playlists.
    assert!(track(s, "Au_Bass_M").playlists.iter().any(|p| p.name == "Au_Bass_M.alt"));
    // Keys: a crossfade between its two clips.
    let keys = track(s, "Au_Keys_St").clips();
    assert_eq!(keys.len(), 2);
    assert!(keys[0].fade_out.len > 0 && keys[1].fade_in.len > 0, "{keys:?}");
    // The tempo map: 120, 132 at bar 2, 112 at bar 3; meters 4/4, 3/4, 6/8.
    let bpm: Vec<(i64, f64)> = s.tempo.tempos().iter().map(|t| (t.tick, t.bpm)).collect();
    assert_eq!(bpm, vec![(0, 120.0), (3_840, 132.0), (7_680, 112.0)]);
    let m: Vec<(i64, u32, u32)> = s.tempo.meters().iter().map(|m| (m.tick, m.numerator, m.denominator)).collect();
    // The fixture puts 6/8 at a quarter note that is not a bar line in 3/4: it moves to the bar.
    assert_eq!(m, vec![(0, 4, 4), (3_840, 3, 4), (6_720, 6, 8)]);
    assert!(v["report_text"].as_str().unwrap().contains("Approximated: the 6/8 meter change is not on a bar line"));
    assert_eq!(s.key_signatures.last().map(|k| k.1.as_str()), Some("F# minor"));
    // Markers: the absolute one in samples, the bar ones in ticks.
    let anchor = s.markers.iter().find(|m| m.name == "Abs Sync Anchor").unwrap();
    assert_eq!(anchor.start, 96_000);
    let bridge = s.markers.iter().find(|m| m.name == "Bridge Mod").unwrap();
    assert_eq!(bridge.start, s.tempo.tick_to_samples(7_680, s.sample_rate));
    // MIDI: notes, CC 1, pitch bend, program changes, poly pressure, SysEx.
    let main = seq(&track(s, "Md_Main").clips()[0]);
    assert_eq!(main.notes.len(), 13);
    let cc = |k: CtrlKind| main.ctrls.iter().filter(|c| c.kind == k).map(|c| (c.tick, c.value)).collect::<Vec<_>>();
    assert_eq!(cc(CtrlKind::Cc(1)), vec![(0, 20), (1920, 110)]);
    assert_eq!(cc(CtrlKind::PitchBend), vec![(0, 0), (960, 4096), (1920, -4096), (2880, 0)]);
    assert_eq!(cc(CtrlKind::Program), vec![(0, 12), (1920, 48)]);
    assert_eq!(cc(CtrlKind::PolyPressure(60)), vec![(0, 45)]);
    assert_eq!(main.sysex.len(), 2);
    // The report names what did not come across.
    let text = v["report_text"].as_str().unwrap();
    for left in ["\"Aux_Verb_St\": D-Verb (insert a): not carried over", "\"Au_Bass_M\": SansAmp PSA-1 (insert a): not carried over", "Master"] {
        assert!(text.contains(left), "{left}: {text}");
    }
    assert!(text.contains("Approximated: \"Au_Gtr_St\": the loop of \"gtr_dl.L\""), "{text}");
    assert!(v["offline"].as_array().unwrap().len() >= 5, "no audio beside it: {v}");
}

#[test]
fn a_session_with_clips_places_the_pro_tools_session_by_timecode() {
    let mut e = crate::demo::demo_engine();
    let before = (e.session().timecode_start, e.session().tempo.clone());
    let v = import_bytes(&mut e, &fixture("CR_min.ptx"), "x.ptx", None, None).unwrap();
    assert_eq!(e.session().timecode_start, before.0, "the session's timecode is kept");
    assert_eq!(e.session().tempo, before.1, "and its tempo map");
    let lead = track(e.session(), "Au_Lead");
    assert_eq!(lead.clips()[0].start, 3600 * 48_000 - before.0);
    assert!(v["report_text"].as_str().unwrap().contains("the session has clips and keeps its own"));
    // `at` puts the session start where asked.
    let mut e = crate::demo::demo_engine();
    let v = import_bytes(&mut e, &fixture("CR_min.ptx"), "x.ptx", None, Some(1000)).unwrap();
    let t = TrackId(v["tracks"][0].as_u64().unwrap());
    assert_eq!(e.session().track(t).unwrap().clips()[0].start, 1000);
}

#[test]
fn bad_files_are_errors_change_nothing_and_never_panic() {
    let mut e = Engine::default();
    let steps = e.undo_history().len();
    assert!(e.execute("file.import_ptx", &json!({"path": "/no/such/file.ptx"})).is_err());
    for bad in [&b""[..], b"not a pro tools session at all", &[0u8; 64][..]] {
        assert!(import_bytes(&mut e, bad, "bad.ptx", None, None).is_err());
    }
    assert_eq!(e.undo_history().len(), steps);
    assert!(e.session().tracks.iter().all(|t| t.clips().is_empty()));
    // Damaged copies: an error or a partial import, never a panic.
    for name in ["CR_min.ptx", "CR_max3.ptx"] {
        let good = fixture(name);
        let mut x: u32 = 11;
        for round in 0..120 {
            let mut b = good.clone();
            if round % 3 == 0 {
                b.truncate((x as usize >> 4) % b.len());
            }
            for _ in 0..4 {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                if let Some(at) = b.len().checked_sub(1).map(|n| (x as usize >> 3) % (n + 1)) {
                    b[at] ^= (x >> 23) as u8 | 1;
                }
            }
            let mut e = Engine::default();
            let _ = import_bytes(&mut e, &b, "c.ptx", None, None);
        }
    }
}
