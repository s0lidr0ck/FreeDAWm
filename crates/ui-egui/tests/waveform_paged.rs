//! A long file played from disk in a saved project keeps only its overview's coarse levels in
//! memory: its waveform draws the coarser level where a fine page isn't loaded yet (never a gap,
//! and the drawing asks to be drawn again), then, once the pages are in, exactly the pixels it
//! draws with its overview held whole in memory, at every zoom.

use egui::{Color32, Pos2, RawInput, Rect, Shape, vec2};
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, encode};
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, Source, SourceAudio, SourceId, TrackKind, disk};
use soundcraft_ui_egui::waveform;
use std::sync::Arc;
use std::time::Duration;

const WAVE: Color32 = Color32::from_rgb(12, 12, 14);
const SR: usize = 48_000;

/// What the waveform paints across a 600×120 band on a 2x screen at `samples_per_px` from
/// `scroll`, and whether it asked to be drawn again soon.
fn paint(clip: &Clip, audio: &SourceAudio, samples_per_px: f64, scroll: i64, power: bool) -> (Vec<Shape>, bool) {
    let mut s = Session::new("t", soundcraft_time::SampleRate::HZ_48000);
    s.edit.zoom.samples_per_px = samples_per_px;
    s.edit.zoom.scroll = scroll;
    if power {
        s.edit.set_flag("waveform.power", true);
    }
    let band = Rect::from_min_size(Pos2::ZERO, vec2(600.0, 120.0));
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(2.0);
    let mut out = ctx.run_ui(RawInput { screen_rect: Some(band), ..Default::default() }, |ui| {
        waveform::draw(ui.painter(), &s, band, clip, audio, 2, band, WAVE);
    });
    out.textures_delta.clear();
    let again = out.viewport_output.values().any(|v| v.repaint_delay <= Duration::from_millis(100));
    (out.shapes.into_iter().map(|c| c.shape).filter(|s| matches!(s, Shape::Path(_) | Shape::Mesh(_))).collect(), again)
}

/// The x extent of what is painted, and every half-point column between that has paint in it
/// (a gap would leave one empty).
fn columns(shapes: &[Shape]) -> std::collections::BTreeSet<i32> {
    let mut xs = std::collections::BTreeSet::new();
    for s in shapes {
        let pts: Vec<Pos2> = match s {
            Shape::Mesh(m) => m.vertices.iter().map(|v| v.pos).collect(),
            Shape::Path(p) => p.points.clone(),
            _ => Vec::new(),
        };
        // A mesh paints the spans between its vertices too: fill each triangle's x range.
        match s {
            Shape::Mesh(m) => {
                for t in m.indices.chunks(3) {
                    let x: Vec<f32> = t.iter().filter_map(|i| m.vertices.get(*i as usize)).map(|v| v.pos.x).collect();
                    let (a, b) = x.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
                    for k in (a * 2.0).ceil() as i32..(b * 2.0).floor() as i32 {
                        xs.insert(k);
                    }
                }
            }
            _ => xs.extend(pts.iter().map(|p| (p.x * 2.0).round() as i32)),
        }
    }
    xs
}

#[test]
fn a_paged_overview_draws_the_pixels_of_one_held_whole() {
    let d = std::env::temp_dir().join(format!("freedawm-waveform-paged-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("Audio Files")).unwrap();
    // Two minutes of stereo with a swell, bursts and single-sample spikes.
    let frames = 120 * SR + 333;
    let chan = |c: usize| -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f32 / SR as f32;
                let spike = if i % 100_003 == 0 { 0.95 } else { 0.0 };
                ((t * 220.0 * (1.0 + c as f32) * std::f32::consts::TAU).sin() * (0.1 + 0.6 * (t * 0.21).sin().abs()) + spike).clamp(-1.0, 1.0)
            })
            .collect()
    };
    let buf = AudioBuffer { sample_rate: SR as u32, channels: vec![chan(0), chan(1)] };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None, bitrate_kbps: 0 };
    let file = d.join("Audio Files").join("Take.wav");
    std::fs::write(&file, encode(&buf, &opts).unwrap()).unwrap();
    drop(buf);
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("A"));
    let id = SourceId(s.alloc());
    s.sources.push(Source {
        id,
        name: "Take".into(),
        path: "Audio Files/Take.wav".into(),
        channels: 2,
        frames: frames as u64,
        sample_rate: SR as u32,
        format: FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    let clip = Clip::audio(ClipId(s.alloc()), "c", id, 0, 0, frames as i64);
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(clip.clone());
    let path = d.join("Song.fdawm");
    std::fs::write(&path, s.to_json().unwrap()).unwrap();
    let mut e = soundcraft_engine::Engine::default();
    e.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    let paged = Arc::clone(e.session().pool.get(id).unwrap());
    assert!(paged.peaks.iter().all(|p| p.is_paged()), "the overview is paged");
    // The same file with its overview held whole.
    let f = disk::DiskFile::open(&file).unwrap();
    let whole = SourceAudio::from_disk(Arc::clone(&f), SourceAudio::from_disk_scanned(f).peaks).unwrap();
    let mut coarse_first = 0;
    for (spp, at) in [(0.3, 0.5), (6.0, 0.2), (40.0, 0.7), (300.0, 0.1), (900.0, 0.6), (4_000.0, 0.3), (20_000.0, 0.0), (60_000.0, 0.0)] {
        let scroll = ((frames as f64 - spp * 600.0).max(0.0) * at) as i64;
        for power in [false, true] {
            let (want, _) = paint(&clip, &whole, spp, scroll, power);
            let (first, again) = paint(&clip, &paged, spp, scroll, power);
            assert!(!want.is_empty());
            let mut got = first.clone();
            if first != want {
                coarse_first += 1;
                // The coarser draw: every column painted (no gap), and a redraw asked for.
                let (c, w) = (columns(&first), columns(&want));
                assert!(w.is_subset(&c), "a gap at {spp} samples a point: {} of {} columns", c.len(), w.len());
                assert!(again, "no redraw asked for while pages load");
                for _ in 0..20 {
                    soundcraft_engine::peak_pages::wait_idle(Duration::from_secs(10));
                    got = paint(&clip, &paged, spp, scroll, power).0;
                    if got == want {
                        break;
                    }
                }
            }
            assert!(got == want, "at {spp} samples a point (power {power}) the paged overview draws other pixels");
        }
    }
    assert!(coarse_first > 0, "some zooms drew the coarser level first");
    let _ = std::fs::remove_dir_all(&d);
}
