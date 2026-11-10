//! Generate Song (`ml.song`) against a stand-in song server that speaks the yue2.cpp / acestep.cpp
//! job protocol (`/health`, `/synth`, `/lm`, `/job`): the song lands on a new audio track at the
//! edit cursor, YuE2's score as MIDI tracks (voices and chords) under it, as one undo step;
//! ACE-Step plans with `/lm` first and renders the planned request; bad params and a missing
//! server are refused with nothing changed; a cancel reaches the server.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::ml::{self, Cancel, Ctx, MlError};
use soundcraft_model::{ClipContent, Session, TrackKind};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const SCORE: &str = "X:1\nM:4/4\nL:1/16\nQ:1/4=120\nV: Vocal\nV: Ins\nK:C\nV: Vocal\n\"C\"C4E4G4c4|\"F\"F4A4c4f4|\nV: Ins\nC,16|F,16|\n";

fn session() -> Engine {
    let mut s = Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() };
    s.tempo.set_tempo(0, 120.0).unwrap();
    s.add_track(TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("Vox"));
    s.edit.selection.start = 48_000;
    Engine::new(s)
}

fn names(e: &Engine) -> Vec<String> {
    e.session().tracks.iter().map(|t| t.name.clone()).collect()
}

/// 2 s of stereo float WAV at 48 kHz.
fn wav() -> Vec<u8> {
    let n = 96_000;
    let ch: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![ch.clone(), ch] };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: soundcraft_audio_io::BitDepth::Float32, ..EncodeOptions::default() };
    soundcraft_audio_io::encode(&buf, &opts).unwrap()
}

/// What the stand-in server saw: (method, path, body).
type Log = Arc<Mutex<Vec<(String, String, String)>>>;

/// A stand-in server on a free port; `polls_before_done` running answers before done; `hold`
/// keeps every job running until it is cancelled.
fn server(polls_before_done: usize, hold: bool) -> (String, Log) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&log);
    std::thread::spawn(move || {
        let mut polls = 0;
        let mut cancelled = false;
        let mut kind = String::new();
        for stream in l.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            if r.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
            let mut len = 0;
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; len];
            let _ = r.read_exact(&mut body);
            let body = String::from_utf8_lossy(&body).to_string();
            seen.lock().unwrap().push((method.clone(), path.clone(), body.clone()));
            let (ctype, out): (String, Vec<u8>) = if path == "/health" {
                ("application/json".into(), br#"{"status":"ok"}"#.to_vec())
            } else if method == "POST" && (path == "/synth" || path == "/lm") {
                kind = path.clone();
                polls = 0;
                ("application/json".into(), br#"{"id":"7"}"#.to_vec())
            } else if path.contains("cancel=1") {
                cancelled = true;
                ("application/json".into(), br#"{"status":"cancelled"}"#.to_vec())
            } else if path.contains("result=1") && kind == "/lm" {
                let req: Value = serde_json::from_str(&seen.lock().unwrap().iter().rev().find(|x| x.1 == "/lm").unwrap().2).unwrap();
                let mut planned = req.clone();
                planned["bpm"] = json!(96);
                planned["keyscale"] = json!("A minor");
                planned["lyrics"] = json!("[Verse]\nplanned words");
                planned["audio_codes"] = json!("1,2,3");
                ("application/json".into(), serde_json::to_vec(&json!([planned])).unwrap())
            } else if path.contains("result=1") {
                let mut b = Vec::new();
                if kind == "/synth" && seen.lock().unwrap().iter().any(|x| x.2.contains("\"style\"")) {
                    let replay = json!({"style": "pop", "abc": SCORE, "seed": 11, "lm_seed": 11});
                    b.extend_from_slice(b"--yb\r\nContent-Type: application/json\r\n\r\n");
                    b.extend_from_slice(&serde_json::to_vec(&replay).unwrap());
                    b.extend_from_slice(b"\r\n");
                }
                b.extend_from_slice(b"--yb\r\nContent-Type: audio/wav\r\n\r\n");
                b.extend_from_slice(&wav());
                b.extend_from_slice(b"\r\n--yb\r\nContent-Type: application/octet-stream\r\n\r\n\0\0\0\0\r\n--yb--\r\n");
                ("multipart/mixed; boundary=yb".into(), b)
            } else if path.starts_with("/job") {
                polls += 1;
                let st = if cancelled {
                    "cancelled"
                } else if hold || polls <= polls_before_done {
                    "running"
                } else {
                    "done"
                };
                ("application/json".into(), format!("{{\"status\":\"{st}\"}}").into_bytes())
            } else {
                ("application/json".into(), br#"{"error":"not found"}"#.to_vec())
            };
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len());
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&out);
        }
    });
    (url, log)
}

#[test]
fn a_yue2_song_lands_with_its_score_as_midi() {
    let (url, log) = server(2, false);
    let mut e = session();
    let v = e
        .execute(
            "ml.song",
            &json!({"engine": "yue2", "server": url, "style": "pop, female voice", "lyrics": "[Verse]\nla la", "seconds": 30, "seed": 11}),
        )
        .unwrap();
    assert_eq!(names(&e), ["Vox", "YuE2 Song", "YuE2 Score Vocal", "YuE2 Score Ins", "YuE2 Score Chords"]);
    assert_eq!(e.undo_label(), Some("Generate Song"));
    assert_eq!(v["seed"], 11);
    assert_eq!(v["score"]["bpm"], 120.0);
    // The request YuE2 got: style, lyrics, the plan, the seeds and float WAV.
    let synth: Value = serde_json::from_str(&log.lock().unwrap().iter().find(|x| x.1 == "/synth").unwrap().2).unwrap();
    assert_eq!((synth["cot"].as_str(), synth["seed"].as_i64(), synth["output_format"].as_str()), (Some("full"), Some(11), Some("wav24")));
    let s = e.session();
    // The audio at the edit cursor, 2 s long.
    let song = s.tracks.iter().find(|t| t.name == "YuE2 Song").unwrap();
    let c = &song.clips()[0];
    assert_eq!((c.start, c.length), (48_000, 96_000));
    // The vocal: C4 E4 G4 C5 then F4 A4 C5 F5, a quarter each at 120 bpm = 960 ticks.
    let vocal = s.tracks.iter().find(|t| t.name == "YuE2 Score Vocal").unwrap();
    let ClipContent::Midi { sequence } = &vocal.clips()[0].content else { panic!("not MIDI") };
    // The clip is the song's 2 s: the first bar fits.
    let pitches: Vec<u8> = sequence.notes.iter().map(|n| n.pitch).collect();
    assert_eq!(pitches, [60, 64, 67, 72]);
    assert_eq!(sequence.notes[1].start, 960);
    // The chords: C (bass C2 + C4 E4 G4) for the first bar.
    let chords = s.tracks.iter().find(|t| t.name == "YuE2 Score Chords").unwrap();
    let ClipContent::Midi { sequence } = &chords.clips()[0].content else { panic!("not MIDI") };
    assert_eq!(sequence.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [36, 60, 64, 67]);
    assert!(e.undo());
    assert_eq!(names(&e), ["Vox"]);
}

#[test]
fn ace_step_plans_then_renders_the_planned_request() {
    let (url, log) = server(1, false);
    let mut e = session();
    let v = e.execute("ml.song", &json!({"engine": "acestep", "server": url, "style": "lofi hip hop", "instrumental": true, "seconds": 20})).unwrap();
    assert_eq!(names(&e), ["Vox", "ACE-Step 1.5 Song"]);
    assert_eq!((v["song"]["bpm"].as_u64(), v["song"]["key"].as_str()), (Some(96), Some("A minor")));
    let calls: Vec<String> = log.lock().unwrap().iter().filter(|x| x.0 == "POST").map(|x| x.1.clone()).collect();
    assert_eq!(calls, ["/lm", "/synth"]);
    let lm: Value = serde_json::from_str(&log.lock().unwrap().iter().find(|x| x.1 == "/lm").unwrap().2).unwrap();
    assert_eq!((lm["caption"].as_str(), lm["lyrics"].as_str()), (Some("lofi hip hop"), Some("[Instrumental]")));
    let synth: Value = serde_json::from_str(&log.lock().unwrap().iter().find(|x| x.1 == "/synth").unwrap().2).unwrap();
    assert_eq!((synth["audio_codes"].as_str(), synth["output_format"].as_str()), (Some("1,2,3"), Some("wav24")));
    // Without the planner: /synth alone.
    let (url, log) = server(0, false);
    let mut e = session();
    e.execute("ml.song", &json!({"engine": "acestep", "server": url, "style": "lofi", "planner": false})).unwrap();
    let calls: Vec<String> = log.lock().unwrap().iter().filter(|x| x.0 == "POST").map(|x| x.1.clone()).collect();
    assert_eq!(calls, ["/synth"]);
}

#[test]
fn bad_params_and_a_missing_server_are_refused() {
    let mut e = session();
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let err = e.execute("ml.song", &json!({"style": "pop", "server": format!("127.0.0.1:{port}")})).unwrap_err().to_string();
    assert!(err.contains("yue-server"), "{err}");
    for p in [
        json!({}),
        json!({"style": "pop", "engine": "suno"}),
        json!({"style": "pop", "seconds": 5}),
        json!({"style": "pop", "engine": "acestep", "bpm": 900}),
        json!({"style": "pop", "engine": "acestep", "score": "X:1"}),
        json!({"style": "pop", "plan": "maybe"}),
        json!({"style": 3}),
        json!({"style": "pop", "server": "ftp://nope"}),
    ] {
        assert!(e.execute("ml.song", &p).is_err(), "{p}");
    }
    assert_eq!(names(&e), ["Vox"]);
    assert!(e.undo_label().is_none());
}

#[test]
fn a_cancel_reaches_the_server_and_changes_nothing() {
    let (url, log) = server(0, true);
    let e = session();
    let mut job = ml::prepare(&e, "ml.song", &json!({"server": url, "style": "pop"})).unwrap();
    assert_eq!(job.title(), "Generate Song — pop");
    let cancel = Cancel::new();
    let ctx = Ctx::new(cancel.clone(), |_, _| {});
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(900));
        c2.cancel();
    });
    let r = job.run(&ctx);
    assert!(matches!(r, Err(MlError::Cancelled)), "{r:?}");
    assert!(log.lock().unwrap().iter().any(|x| x.1.contains("cancel=1")), "the server was told");
    assert_eq!(names(&e), ["Vox"]);
}
