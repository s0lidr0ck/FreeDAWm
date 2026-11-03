//! DAWproject import and export: the official example projects (transcribed from the spec repo's
//! tests), a Studio One–style song, timing exactness against the tempo map, the round trip, the
//! placeholder state blobs, the report, and hostile files.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions};
use soundcraft_engine::Engine;
use soundcraft_engine::dawproject::archive::{Archive, ArchiveWriter};
use soundcraft_engine::dawproject::import::{Imported, Installed};
use soundcraft_engine::dawproject::xml::Element;
use soundcraft_engine::dawproject::{self, Fate, Report};
use soundcraft_model::{AutoParam, ClipContent, Route, Session, TrackKind};

const SR: f64 = 44_100.0;

fn fixture(name: &str) -> String {
    let dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("tests/fixtures/dawproject");
    std::fs::read_to_string(dir.join(name)).unwrap()
}

/// The spec repo's test sample's shape (44.1 kHz stereo, 3.097 s), synthesised.
fn white_glasses() -> Vec<u8> {
    let frames = (3.097 * SR).round() as usize;
    let ch: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.03).sin() * 0.25).collect();
    let buf = AudioBuffer { sample_rate: 44_100, channels: vec![ch.clone(), ch] };
    soundcraft_audio_io::encode(&buf, &EncodeOptions { bit_depth: BitDepth::Int16, dither: false, ..EncodeOptions::default() }).unwrap()
}

fn archive(xml: &str, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut w = ArchiveWriter::new(std::io::Cursor::new(Vec::new()));
    w.add("project.xml", xml.as_bytes()).unwrap();
    for (name, bytes) in files {
        w.add(name, bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn nothing_installed() -> Installed {
    Box::new(|id: &str| Some(soundcraft_dsp::plugin_info(id).is_some()))
}

fn import(bytes: &[u8]) -> (Session, Report) {
    let Imported { mut session, mut report, mut pending } =
        dawproject::import_bytes(bytes, "Song", "Song.dawproject", nothing_installed(), &mut |_| {}).unwrap();
    dawproject::finish_import(&mut session, &mut report, &mut pending);
    (session, report)
}

fn track<'a>(s: &'a Session, name: &str) -> &'a soundcraft_model::Track {
    s.tracks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no track {name}: {:?}", s.tracks.iter().map(|t| &t.name).collect::<Vec<_>>()))
}

fn beats_to_samples(s: &Session, beats: f64) -> i64 {
    s.tempo.ticks_f_to_samples_f(beats * 960.0, s.sample_rate).round() as i64
}

fn has(r: &Report, fate: Fate, what: &str) -> bool {
    r.items.iter().any(|i| i.fate == fate && (i.what.contains(what) || i.why.contains(what)))
}

#[test]
fn bitwig_warped_audio_with_fades_lands_exactly() {
    let bytes = archive(&fixture("bitwig-audio-warped-fades.xml"), &[("white-glasses.wav", white_glasses())]);
    let (s, r) = import(&bytes);
    assert_eq!(s.sample_rate.hz(), 44_100);
    assert_eq!(s.tempo.tempos()[0].bpm, 155.0);
    let a = track(&s, "Audio");
    assert_eq!(a.kind, TrackKind::Audio);
    assert_eq!(a.mixer.output, Route::Main);
    assert_eq!(track(&s, "Master").kind, TrackKind::Master);
    let c = &a.clips()[0];
    assert_eq!(c.start, 0);
    assert_eq!(c.length, (8.0 * 60.0 / 155.0 * SR).round() as i64, "8 beats at 155 BPM");
    assert!(matches!(c.content, ClipContent::Audio { offset: 0, .. }));
    let quarter = (0.25 * 60.0 / 155.0 * SR).round() as i64;
    assert_eq!((c.fade_in.len, c.fade_out.len), (quarter, quarter));
    assert!(!has(&r, Fate::Changed, "time-stretched"), "warps at the file's own speed are not a stretch: {r:?}");
}

#[test]
fn bitwig_raw_seconds_clip_keeps_its_offset() {
    let bytes = archive(&fixture("bitwig-audio-raw-seconds.xml"), &[("white-glasses.wav", white_glasses())]);
    let (s, _) = import(&bytes);
    let c = &track(&s, "Audio").clips()[0];
    assert_eq!(c.start, 0);
    assert!(matches!(c.content, ClipContent::Audio { offset: 44_100, .. }), "{:?}", c.content);
    assert_eq!(c.length, (2.097 * SR).round() as i64, "plays from 1.0 s to the play stop");
}

#[test]
fn bitwig_cc_automation_in_clips_becomes_controller_events() {
    let (s, _) = import(&archive(&fixture("bitwig-midi-cc1-in-clips.xml"), &[]));
    let t = track(&s, "Notes");
    assert!(t.kind.is_midi());
    let ClipContent::Midi { sequence } = &t.clips()[0].content else { panic!() };
    let cc: Vec<_> = sequence.ctrls.iter().filter(|c| c.kind == soundcraft_midi::CtrlKind::Cc(1)).collect();
    assert_eq!((cc[0].tick, cc[0].value), (0, 0));
    assert!(cc.iter().any(|c| c.tick == 4 * 960 && c.value == 127), "full at beat 4");
    assert!(cc.iter().any(|c| c.tick == 3 * 960 + 960 / 2 && c.value > 64 && c.value < 127), "ramping between 3 and 4: {cc:?}");
    assert!(cc.iter().filter(|c| c.tick > 2 * 960 && c.tick < 3 * 960).count() == 0, "flat between 2 and 3: no events");
    assert!(cc.iter().all(|c| c.tick <= 8 * 960));
    assert_eq!(t.clips()[0].length, beats_to_samples(&s, 8.0));
}

#[test]
fn bitwig_complex_project_tracks_alias_clips_automation_markers() {
    let (s, r) = import(&archive(&fixture("bitwig-complex.xml"), &[]));
    assert_eq!(s.sample_rate.hz(), 48_000, "no audio: 48 kHz");
    let names: Vec<&str> = s.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Track 1", "Track 2", "Track 3", "Master"], "order kept, the master last");
    let t1 = track(&s, "Track 1");
    assert_eq!(t1.color, [0, 0, 0]);
    assert_eq!(track(&s, "Track 2").color, [0x11, 0x11, 0x11]);
    assert_eq!(t1.clips().len(), 2);
    let alias = &t1.clips()[1];
    assert_eq!(alias.start, beats_to_samples(&s, 32.0));
    let (ClipContent::Midi { sequence: a }, ClipContent::Midi { sequence: b }) = (&t1.clips()[0].content, &alias.content) else { panic!() };
    assert_eq!(a, b, "an alias clip plays the same notes");
    assert_eq!(a.notes.len(), 8);
    assert_eq!(a.notes[1].start, 480);
    assert_eq!(a.notes[1].length, 480);
    assert_eq!(a.notes[0].velocity, (0.8f64 * 127.0).round() as u8);
    assert_eq!(a.notes[0].release_velocity, 64, "0.5 of 127, rounded");
    let ClipContent::Midi { sequence: t3 } = &track(&s, "Track 3").clips()[0].content else { panic!() };
    assert_eq!(t3.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [36, 48, 60, 36, 48, 60, 36, 48]);
    let vol = t1.lane(&AutoParam::Volume).unwrap();
    assert_eq!(vol.points.len(), 2);
    assert_eq!((vol.points[0].at, vol.points[0].value), (0, -144.0));
    assert_eq!((vol.points[1].at, vol.points[1].value), (beats_to_samples(&s, 8.0), 0.0));
    let markers: Vec<(&str, i64)> = s.markers.iter().map(|m| (m.name.as_str(), m.start)).collect();
    assert_eq!(markers, [("Verse", 0), ("Chorus", 12 * 48_000)]);
    // The master's Limiter has no plugin id and its state file is missing.
    let m = track(&s, "Master");
    assert_eq!(m.mixer.inserts.len(), 1);
    assert!(!m.mixer.inserts[0].active, "a placeholder is inactive");
    assert!(m.mixer.inserts[0].foreign.is_some());
    assert!(has(&r, Fate::Placeholder, "Limiter"), "{}", r.to_text());
    assert!(has(&r, Fate::Changed, "state file is missing"), "{}", r.to_text());
}

fn studio_one_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("Audio/white-glasses.wav", white_glasses()),
        (
            "Plugins/Pro-Q 3.vstpreset",
            soundcraft_vst3_host::vstpreset::write_vstpreset("ABCDEF0123456789ABCDEF0123456789", b"proq-comp", b"proq-ctrl").unwrap(),
        ),
        ("Plugins/Pro EQ3.preset", b"PRESONUS\x00\x01\x02 state that only Studio One reads \xff".to_vec()),
        ("Plugins/Synth.clap-preset", b"clap state".to_vec()),
    ]
}

fn studio_one() -> (Session, Report, Vec<u8>) {
    let bytes = archive(&fixture("studio-one-style.xml"), &studio_one_files());
    let (s, r) = import(&bytes);
    (s, r, bytes)
}

#[test]
fn studio_one_song_structure_and_mixer() {
    let (s, r, _) = studio_one();
    assert_eq!(r.application, "Studio One 6.5.2");
    let names: Vec<&str> = s.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Drums", "Kick", "Keys", "Vox", "FX", "Main"]);
    let drums = track(&s, "Drums");
    let kick = track(&s, "Kick");
    assert_eq!(drums.kind, TrackKind::Folder);
    assert_eq!(kick.folder, Some(drums.id));
    let Route::Bus(fold_bus) = drums.mixer.input else { panic!("a folder sums through its bus") };
    assert_eq!(kick.mixer.output, Route::Bus(fold_bus), "the kick goes to its folder");
    assert!((drums.mixer.volume_db + 6.0).abs() < 1e-3);
    assert_eq!(kick.format, soundcraft_model::ChannelFormat::Mono);
    assert!((kick.mixer.volume_db + 6.0).abs() < 1e-6, "dB unit read as dB");
    assert!((kick.mixer.pan[0] + 0.5).abs() < 1e-6, "0.25 normalized is half left");
    let fx = track(&s, "FX");
    assert_eq!(fx.kind, TrackKind::Aux);
    let Route::Bus(fx_bus) = fx.mixer.input else { panic!() };
    let send = kick.mixer.sends[0].as_ref().unwrap();
    assert_eq!(send.target, Route::Bus(fx_bus));
    assert!((send.level_db + 12.0).abs() < 1e-3);
    assert!(!send.pre_fader);
    assert_eq!(track(&s, "Main").kind, TrackKind::Master);
    let keys = track(&s, "Keys");
    assert_eq!(keys.kind, TrackKind::Instrument);
    assert!(keys.mixer.mute && keys.mixer.solo);
    assert_eq!(keys.mixer.pan, vec![0.0, 1.0], "a stereo track's balance right narrows the left side");
    let inst = keys.instrument.as_ref().unwrap();
    assert_eq!(inst.plugin, "clap:com.example.synth");
    assert_eq!(inst.state_bytes().as_deref(), Some(&b"clap state"[..]));
    assert!(!inst.active, "not installed: an inactive placeholder keeping its state");
    // Kick's chain: Pro-Q 3 (VST3, not installed here) then Studio One's Pro EQ3.
    let proq = &kick.mixer.inserts[0];
    let id = if cfg!(windows) { "vst3:01EFCDAB45238967ABCDEF0123456789" } else { "vst3:ABCDEF0123456789ABCDEF0123456789" };
    assert_eq!(proq.plugin, id);
    let blob = proq.state_bytes().unwrap();
    assert_eq!(soundcraft_vst3_host::vstpreset::unpack_state(&blob), Some((b"proq-comp".to_vec(), b"proq-ctrl".to_vec())));
    let proeq = &kick.mixer.inserts[1];
    assert!(proeq.plugin.starts_with("dawproject:"));
    assert!(proeq.bypass, "Enabled false");
    assert_eq!(proeq.preset, "Pro EQ3");
    assert!(has(&r, Fate::Placeholder, "Pro EQ3"));
    assert!(has(&r, Fate::Placeholder, "Pro-Q 3"));
    assert!(has(&r, Fate::Placeholder, "Old Chorus"), "VST2");
    assert!(has(&r, Fate::Dropped, "Clip slot"));
    assert!(has(&r, Fate::Dropped, "scenes"));
    assert!(has(&r, Fate::Dropped, "channelController automation"));
    assert!(has(&r, Fate::Dropped, "Band 1 Gain"), "plugin automation without the plugin: {}", r.to_text());
}

#[test]
fn studio_one_clips_tempo_and_automation_are_exact() {
    let (s, _, _) = studio_one();
    assert_eq!(s.sample_rate.hz(), 44_100);
    // Tempo: 120 to beat 16, a ramp to 140 at beat 24; 3/4 from beat 32.
    let t = s.tempo.tempos();
    assert_eq!(t.len(), 3, "{t:?}");
    assert!(!t[0].ramp && t[1].ramp && !t[2].ramp, "{t:?}");
    assert_eq!((t[1].tick, t[2].tick, t[2].bpm), (16 * 960, 24 * 960, 140.0));
    assert_eq!(s.tempo.meters()[1].numerator, 3);
    assert_eq!(s.tempo.meters()[1].tick, 32 * 960);
    // The kick's looped clip: 3 s from beat 4, a 1 s loop from 0.5 s, unrolled.
    let kick = track(&s, "Kick");
    let clips = kick.clips();
    assert_eq!(clips.len(), 3, "{clips:?}");
    for (i, c) in clips.iter().enumerate() {
        assert_eq!(c.start, 88_200 + 44_100 * i as i64);
        assert_eq!(c.length, 44_100);
        assert!(matches!(c.content, ClipContent::Audio { offset: 22_050, .. }));
        assert_eq!(c.loop_id, clips[0].loop_id);
        assert_eq!(c.name, "Kick loop");
    }
    assert!(clips[0].loop_id.is_some());
    // Vox: two take lanes; the first is active, the other a playlist.
    let vox = track(&s, "Vox");
    assert_eq!(vox.playlists.len(), 2);
    let take2 = &vox.clips()[0];
    assert_eq!((take2.start, take2.length), (176_400, 88_200));
    assert!((take2.gain_db + 6.0206).abs() < 1e-3, "{}", take2.gain_db);
    assert_eq!((take2.fade_in.len, take2.fade_out.len), (441, 22_050));
    let take1 = &vox.playlists[1].clips[0];
    assert!(matches!(take1.content, ClipContent::Audio { offset: 44_100, .. }));
    // Keys: the riff's 1-beat loop over 2 beats, and the nested clip trimmed by its parent.
    let keys = track(&s, "Keys");
    let ClipContent::Midi { sequence } = &keys.clips()[0].content else { panic!() };
    assert_eq!(sequence.notes.iter().map(|n| (n.start, n.pitch)).collect::<Vec<_>>(), [(0, 60), (480, 64), (960, 60), (1440, 64)]);
    assert_eq!((sequence.notes[0].velocity, sequence.notes[0].release_velocity, sequence.notes[1].release_velocity), (64, 32, 64));
    let inner = &keys.clips()[1];
    assert_eq!(inner.name, "Inner");
    assert_eq!((inner.start, inner.length), (88_200, 22_050));
    let ClipContent::Midi { sequence } = &inner.content else { panic!() };
    assert_eq!(sequence.notes.iter().map(|n| (n.start, n.pitch, n.channel)).collect::<Vec<_>>(), [(480, 69, 1)]);
    // Automation: the kick's pan holds hard left until beat 4, then is hard right; mute at beat 16.
    let pan = kick.lane(&AutoParam::Pan(0)).unwrap();
    assert_eq!(pan.points.iter().map(|p| (p.at, p.value)).collect::<Vec<_>>(), [(0, -1.0), (88_199, -1.0), (88_200, 1.0)]);
    let mute = kick.lane(&AutoParam::Mute).unwrap();
    assert_eq!(mute.points.last().map(|p| (p.at, p.value)), Some((beats_to_samples(&s, 16.0), 1.0)));
    assert!(kick.auto_lanes.contains(&"pan".to_string()));
    // Markers on the tempo map.
    let verse = s.markers.iter().find(|m| m.name == "Verse").unwrap();
    assert_eq!(verse.start, 352_800);
    assert_eq!(s.markers.iter().find(|m| m.name == "Intro").unwrap().color, Some([255, 0, 0]));
    // The report is in the Notes tab.
    assert!(s.notes.iter().any(|n| n.tags.contains(&"dawproject".to_string()) && n.text.contains("Imported Song.dawproject")));
}

/// Track name → (kind, folder name, output as a track name or "main").
fn layout(s: &Session) -> Vec<(String, TrackKind, Option<String>, String)> {
    let reader = |b| s.tracks.iter().find(|t| t.mixer.input == Route::Bus(b)).map(|t| t.name.clone()).unwrap_or_default();
    s.tracks
        .iter()
        .map(|t| {
            let out = match &t.mixer.output {
                Route::Main => "main".to_string(),
                Route::Bus(b) => reader(*b),
                other => format!("{other:?}"),
            };
            (t.name.clone(), t.kind, t.folder.and_then(|f| s.track(f)).map(|f| f.name.clone()), out)
        })
        .collect()
}

#[test]
fn export_then_import_gives_back_everything_mapped() {
    let (a, _, _) = studio_one();
    let (w, done) = dawproject::export::export(&a, std::io::Cursor::new(Vec::new())).unwrap();
    assert_eq!(done.tracks, a.tracks.len());
    assert_eq!(done.audio_files, 1, "one source, written once");
    let (b, r) = import(&w.into_inner());
    assert_eq!(layout(&a), layout(&b));
    for (ta, tb) in a.tracks.iter().zip(&b.tracks) {
        assert_eq!(ta.color, tb.color, "{}", ta.name);
        assert!((ta.mixer.volume_db - tb.mixer.volume_db).abs() < 1e-3, "{}", ta.name);
        assert_eq!(ta.mixer.mute, tb.mixer.mute);
        assert_eq!(ta.mixer.solo, tb.mixer.solo);
        for (pa, pb) in ta.mixer.pan.iter().zip(&tb.mixer.pan) {
            assert!((pa - pb).abs() < 1e-5, "{}: {:?} {:?}", ta.name, ta.mixer.pan, tb.mixer.pan);
        }
        assert_eq!(ta.playlists.len(), tb.playlists.len(), "{}", ta.name);
        for (pa, pb) in ta.playlists.iter().zip(&tb.playlists) {
            assert_eq!(pa.clips.len(), pb.clips.len(), "{}", ta.name);
            for (ca, cb) in pa.clips.iter().zip(&pb.clips) {
                assert_eq!((ca.start, ca.length, &ca.name, ca.muted), (cb.start, cb.length, &cb.name, cb.muted), "{}", ta.name);
                assert_eq!((ca.fade_in.len, ca.fade_out.len), (cb.fade_in.len, cb.fade_out.len));
                assert!((ca.gain_db - cb.gain_db).abs() < 1e-3);
                match (&ca.content, &cb.content) {
                    (ClipContent::Audio { offset: x, .. }, ClipContent::Audio { offset: y, .. }) => assert_eq!(x, y),
                    (ClipContent::Midi { sequence: x }, ClipContent::Midi { sequence: y }) => assert_eq!(x.notes, y.notes),
                    other => panic!("{other:?}"),
                }
            }
        }
        for la in &ta.automation {
            let lb = tb.lane(&la.param).unwrap_or_else(|| panic!("{}: lane {:?}", ta.name, la.param));
            assert_eq!(la.points.len(), lb.points.len(), "{} {:?}", ta.name, la.param);
            for (x, y) in la.points.iter().zip(&lb.points) {
                assert_eq!(x.at, y.at);
                assert!((x.value - y.value).abs() < 1e-3);
            }
        }
        assert_eq!(ta.mixer.inserts.len(), tb.mixer.inserts.len(), "{}", ta.name);
        for (ia, ib) in ta.mixer.inserts.iter().zip(&tb.mixer.inserts) {
            assert_eq!((&ia.plugin, ia.bypass, &ia.state), (&ib.plugin, ib.bypass, &ib.state), "{}", ta.name);
        }
        assert_eq!(ta.instrument.as_ref().map(|i| (&i.plugin, &i.state)), tb.instrument.as_ref().map(|i| (&i.plugin, &i.state)));
    }
    let sends = |s: &Session| track(s, "Kick").mixer.sends[0].as_ref().map(|x| x.level_db).unwrap();
    assert!((sends(&a) - sends(&b)).abs() < 1e-3);
    assert_eq!(a.tempo, b.tempo);
    assert_eq!(
        a.markers.iter().map(|m| (&m.name, m.start, m.color)).collect::<Vec<_>>(),
        b.markers.iter().map(|m| (&m.name, m.start, m.color)).collect::<Vec<_>>()
    );
    assert_eq!(r.application.split(' ').next(), Some("FreeDAWm"));
    // The audio came back sample for sample (float WAV).
    let src = |s: &Session| {
        let id = track(s, "Vox").clips()[0].source().unwrap();
        s.pool.get(id).unwrap().buffer().channels[0][1000..1010].to_vec()
    };
    assert_eq!(src(&a), src(&b));
}

#[test]
fn freedawm_modules_round_trip_whole() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "name": "Gtr", "format": "stereo"})).unwrap();
    let t = e.session().tracks[0].id;
    e.execute("mix.insert", &json!({"track": t.0, "plugin": "gain"})).unwrap();
    let m = e.session().tracks[0].mixer.inserts[0].id;
    {
        let s = e.session_mut();
        let tr = s.track_mut(t).unwrap();
        tr.mixer.inserts[0].bypass = true;
        let lane = tr.lane_mut(&AutoParam::Plugin { module: m, param: "gain".into() });
        lane.set_point(0, -6.0);
        lane.set_point(48_000, 3.0);
    }
    let a = e.session().clone();
    let (w, done) = dawproject::export::export(&a, std::io::Cursor::new(Vec::new())).unwrap();
    assert!(done.notes.iter().any(|n| n.contains("FreeDAWm devices")), "{:?}", done.notes);
    let (b, r) = import(&w.into_inner());
    let (ia, ib) = (&track(&a, "Gtr").mixer.inserts[0], &track(&b, "Gtr").mixer.inserts[0]);
    assert_eq!((&ia.plugin, ia.bypass, &ia.params), (&ib.plugin, ib.bypass, &ib.params));
    assert!(ib.foreign.is_none() && ib.active);
    let lane = track(&b, "Gtr").automation.iter().find(|l| matches!(&l.param, AutoParam::Plugin { param, .. } if param == "gain")).unwrap();
    assert_eq!(lane.points.iter().map(|p| p.at).collect::<Vec<_>>(), [0, 48_000]);
    assert!((lane.points[0].value + 6.0).abs() < 1e-3 && (lane.points[1].value - 3.0).abs() < 1e-3, "{:?}", lane.points);
    assert_eq!(r.count(Fate::Placeholder), 0, "{}", r.to_text());
}

/// The state file of the device named `name` in an exported archive.
fn exported_state(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut a = Archive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let xml = String::from_utf8(a.read("project.xml", 1 << 24).unwrap()).unwrap();
    let p = Element::parse(&xml).unwrap();
    let mut stack = vec![&p];
    while let Some(e) = stack.pop() {
        if e.attr("deviceName") == Some(name) {
            let path = e.child("State").and_then(|s| s.attr("path")).unwrap().to_string();
            return a.read(&path, 1 << 24).unwrap();
        }
        stack.extend(e.children.iter());
    }
    panic!("no device {name}");
}

#[test]
fn placeholders_write_their_state_back_byte_for_byte() {
    let (s, _, _) = studio_one();
    let (w, _) = dawproject::export::export(&s, std::io::Cursor::new(Vec::new())).unwrap();
    let out = w.into_inner();
    let files = studio_one_files();
    let orig = |n: &str| files.iter().find(|(p, _)| *p == n).unwrap().1.clone();
    assert_eq!(exported_state(&out, "Pro EQ3"), orig("Plugins/Pro EQ3.preset"));
    assert_eq!(exported_state(&out, "Pro-Q 3"), orig("Plugins/Pro-Q 3.vstpreset"), "an unchanged VST3 state goes back as it came");
    assert_eq!(exported_state(&out, "Synth"), orig("Plugins/Synth.clap-preset"));
    // And through a second round trip.
    let (s2, _) = import(&out);
    let (w2, _) = dawproject::export::export(&s2, std::io::Cursor::new(Vec::new())).unwrap();
    assert_eq!(exported_state(&w2.into_inner(), "Pro EQ3"), orig("Plugins/Pro EQ3.preset"));
}

#[test]
fn a_changed_plugin_state_is_written_as_a_new_preset() {
    let (mut s, _, _) = studio_one();
    let kick = s.tracks.iter().position(|t| t.name == "Kick").unwrap();
    let blob = soundcraft_vst3_host::vstpreset::pack_state(b"new-comp", b"").unwrap();
    s.tracks[kick].mixer.inserts[0].set_state_bytes(&blob);
    let (w, _) = dawproject::export::export(&s, std::io::Cursor::new(Vec::new())).unwrap();
    let preset = soundcraft_vst3_host::vstpreset::read_vstpreset(&exported_state(&w.into_inner(), "Pro-Q 3")).unwrap();
    assert_eq!(preset.component, b"new-comp");
    assert_eq!(preset.class_id, "ABCDEF0123456789ABCDEF0123456789", "the canonical class id, as the other DAW wrote it");
}

#[test]
fn the_commands_import_in_the_background_shape_and_export() {
    let dir = std::env::temp_dir().join(format!("sc-dawproject-cmd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("Song.dawproject");
    std::fs::write(&src, archive(&fixture("studio-one-style.xml"), &studio_one_files())).unwrap();
    let mut e = Engine::default();
    let v = e.execute("file.import_dawproject", &json!({"path": src.to_string_lossy()})).unwrap();
    assert_eq!(v["name"], "Song");
    assert_eq!(v["tracks"], 6);
    assert!(v["report_text"].as_str().unwrap().contains("Imported Song.dawproject from Studio One 6.5.2"));
    assert!(v["report"]["items"].as_array().unwrap().iter().any(|i| i["fate"] == "placeholder"));
    assert!(e.path.is_none(), "a new, unsaved session");
    assert!(e.is_dirty(), "closing it asks to save");
    assert!(e.session().notes.iter().any(|n| n.tags.contains(&"dawproject".to_string())));
    let out = dir.join("Out");
    let v = e.execute("file.export_dawproject", &json!({"path": out.to_string_lossy()})).unwrap();
    let written = dir.join("Out.dawproject");
    assert_eq!(v["path"], written.to_string_lossy().as_ref());
    assert!(written.exists());
    assert!(!dir.join("Out.dawproject.tmp").exists());
    assert!(e.execute("file.export_dawproject", &json!({"path": written.to_string_lossy()})).is_err(), "no silent replace");
    assert!(e.execute("file.export_dawproject", &json!({"path": written.to_string_lossy(), "replace": true})).is_ok());
    // A progress report per phase, in order, through the open job.
    let mut phases = Vec::new();
    let req = soundcraft_engine::open_job::OpenRequest::from_command(&e, "file.import_dawproject", &json!({"path": written.to_string_lossy()}))
        .unwrap()
        .unwrap();
    let loaded = soundcraft_engine::open_job::load_session(req, &mut |p| phases.push(p.phase)).unwrap();
    assert!(phases.contains(&soundcraft_engine::open_job::OpenPhase::Audio));
    assert!(loaded.import.is_some());
    assert!(e.execute("file.import_dawproject", &json!({"path": dir.join("nope.dawproject").to_string_lossy()})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// xorshift64*: deterministic noise for the hostile-file tests.
fn noise(seed: &mut u64) -> u64 {
    *seed ^= *seed >> 12;
    *seed ^= *seed << 25;
    *seed ^= *seed >> 27;
    seed.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

#[test]
fn hostile_archives_and_xml_never_panic() {
    let good = archive(&fixture("studio-one-style.xml"), &studio_one_files());
    let xml = fixture("studio-one-style.xml");
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let try_import = |bytes: &[u8]| {
        let _ = dawproject::import_bytes(bytes, "x", "x.dawproject", nothing_installed(), &mut |_| {});
    };
    // Not zips, truncated zips, damaged zips.
    try_import(b"");
    try_import(b"PK\x03\x04garbage");
    for cut in [10, 100, good.len() / 2, good.len() - 1] {
        try_import(&good[..cut]);
    }
    for _ in 0..200 {
        let mut b = good.clone();
        for _ in 0..8 {
            let i = (noise(&mut seed) as usize) % b.len();
            b[i] = noise(&mut seed) as u8;
        }
        try_import(&b);
    }
    // Damaged XML in a sound zip: flipped bytes, cut short, numbers made hostile.
    let audio = studio_one_files();
    for round in 0..300 {
        let mut x = xml.clone().into_bytes();
        match round % 3 {
            0 => {
                for _ in 0..6 {
                    let i = (noise(&mut seed) as usize) % x.len();
                    x[i] = b"<>\"/ =x0-9eE.#&;"[(noise(&mut seed) % 16) as usize];
                }
            }
            1 => x.truncate((noise(&mut seed) as usize) % x.len()),
            _ => {
                let s = String::from_utf8_lossy(&x).into_owned();
                let hostile = ["1e308", "-1e308", "inf", "-inf", "NaN", "-0", "99999999999999999999", "-5"][(noise(&mut seed) % 8) as usize];
                let s = s.replacen("\"0.0\"", &format!("\"{hostile}\""), 1 + (noise(&mut seed) % 5) as usize).replacen(
                    "\"4.0\"",
                    &format!("\"{hostile}\""),
                    2,
                );
                x = s.into_bytes();
            }
        }
        let text = String::from_utf8_lossy(&x).into_owned();
        try_import(&archive(&text, &audio));
    }
    // Deep nesting and huge loops.
    let deep = format!("<Project version=\"1.0\"><Application name=\"a\" version=\"1\"/>{}</Project>", "<Structure><Track>".repeat(400));
    try_import(&archive(&deep, &[]));
    let looped = xml
        .replace("loopStart=\"0.0\" loopEnd=\"1.0\"", "loopStart=\"0.0\" loopEnd=\"0.0000001\"")
        .replace("duration=\"2.0\" loopStart", "duration=\"100000000\" loopStart");
    let (s, _) = import(&archive(&looped, &audio));
    assert!(s.tracks.iter().all(|t| t.clips().len() <= 5000));
}
