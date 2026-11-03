//! Shared set-up for the Vocal Editor tests: a synthetic sung take on a "Lead Vox" track, egui
//! frames with real input, and the editor's stored control rects.
#![allow(dead_code)]

use egui::{Event, Id, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, TrackId, TrackKind};
use soundcraft_time::Samples;
use soundcraft_ui_egui::{Services, SoundApp};
use std::f64::consts::TAU;

pub const RATE: u32 = 48_000;
const FORMANTS: [f64; 3] = [700.0, 1200.0, 2600.0];
const BANDWIDTHS: [f64; 3] = [100.0, 110.0, 160.0];

/// `secs` of a harmonic series at `f0` shaped by vowel formants, at an RMS of `db` dBFS.
pub fn vowel(f0: f64, secs: f64, db: f64) -> Vec<f32> {
    let n = (secs * f64::from(RATE)) as usize;
    let amp = |k: usize| {
        let f = f0 * k as f64;
        let res = |i: usize, a: f64| a / (1.0 + ((f - FORMANTS[i]) / BANDWIDTHS[i]).powi(2));
        res(0, 1.0) + res(1, 0.6) + res(2, 0.3) + 0.01 / k as f64
    };
    let ks: Vec<(usize, f64)> = (1..=60).filter(|k| f0 * (*k as f64) < f64::from(RATE) / 2.0).map(|k| (k, amp(k))).collect();
    let raw: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            ks.iter().map(|(k, a)| a * (TAU * f0 * *k as f64 * t).sin()).sum::<f64>()
        })
        .collect();
    let rms = (raw.iter().map(|v| v * v).sum::<f64>() / n.max(1) as f64).sqrt();
    let g = 10f64.powf(db / 20.0) / rms.max(1e-12);
    raw.iter().map(|v| (v * g) as f32).collect()
}

pub fn cents(hz: f64, c: f64) -> f64 {
    hz * 2f64.powf(c / 1200.0)
}

pub fn gap(secs: f64) -> impl Iterator<Item = f32> {
    std::iter::repeat_n(0.0, (secs * f64::from(RATE)) as usize)
}

/// Plan 2's two-note take: 0.5 s at 220 Hz −30 c, 0.2 s of silence, 0.5 s at 247 Hz +20 c.
pub fn take2() -> Vec<f32> {
    let mut x = vowel(cents(220.0, -30.0), 0.5, -21.0);
    x.extend(gap(0.2));
    x.extend(vowel(cents(247.0, 20.0), 0.5, -21.0));
    x
}

/// Three notes 0.2 s apart at −27, −21 and −15 dB.
pub fn take3() -> Vec<f32> {
    let mut x = vowel(cents(349.228, 40.0), 0.5, -27.0);
    x.extend(gap(0.2));
    x.extend(vowel(cents(220.0, -30.0), 0.5, -21.0));
    x.extend(gap(0.2));
    x.extend(vowel(cents(247.0, 20.0), 0.5, -15.0));
    x
}

/// Places `take` as a clip at `start` on `t`.
pub fn place(s: &mut Session, t: TrackId, take: Vec<f32>, start: Samples) -> ClipId {
    let len = take.len();
    let src = soundcraft_engine::io::add_source(s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![take] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, Clip::audio(id, "Vox", src, 0, start, len as Samples));
    id
}

pub struct Setup {
    pub ctx: egui::Context,
    pub app: SoundApp,
    pub track: TrackId,
    pub clips: Vec<ClipId>,
}

/// A mono "Lead Vox" track with one clip per take (each at its start), analysed when `analyse`,
/// the Edit view at 0 zoomed to show 3 s, the Vocal Editor open on it.
pub fn setup_with(takes: Vec<(Vec<f32>, Samples)>, analyse: bool) -> Setup {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let clips: Vec<ClipId> = takes.into_iter().map(|(take, at)| place(&mut s, t, take, at)).collect();
    let mut e = Engine::new(s);
    if analyse {
        soundcraft_engine::vocal::run_until_idle(&mut e, &clips).unwrap();
    }
    let mut app = SoundApp::new(e, None, Services::default());
    app.run("edit.select", json!({"tracks": [t.0]})).unwrap();
    app.ui.show_vocal_editor = true;
    let ctx = egui::Context::default();
    for _ in 0..4 {
        frame(&ctx, &mut app, vec![]);
    }
    // A tall lower dock, so the whole Tuning panel shows.
    let mut path = Vec::new();
    if let Some(root) = app.ui.docking.layout.root.as_ref()
        && vocal_split(root, &mut path)
    {
        app.run("ui.dock", json!({"operation": "resizeSplit", "path": path, "size": {"FixedSecond": 720.0}})).unwrap();
    }
    // 3 s across the Edit window's timeline.
    let w = (app.edit_layout.timeline[2] - app.edit_layout.timeline[0]).max(100.0);
    app.engine.session_mut().edit.zoom.samples_per_px = 3.0 * f64::from(RATE) / f64::from(w);
    app.engine.session_mut().edit.zoom.scroll = 0;
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    Setup { ctx, app, track: t, clips }
}

/// [`setup_with`] on [`take2`] at 0.25 s, analysed.
pub fn setup() -> Setup {
    setup_with(vec![(take2(), i64::from(RATE) / 4)], true)
}

pub fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    frame_m(ctx, app, events, Modifiers::NONE);
}

/// One frame with `modifiers` held.
pub fn frame_m(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>, modifiers: Modifiers) {
    let mut all = vec![Event::ModifiersChanged(modifiers)];
    all.extend(events);
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1200.0))), events: all, ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

pub fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
}

/// Presses `k` with `m` while the pointer is at `at`.
pub fn press(ctx: &egui::Context, app: &mut SoundApp, at: Pos2, k: Key, m: Modifiers) {
    frame_m(ctx, app, vec![Event::PointerMoved(at)], m);
    frame_m(ctx, app, key(k, m), m);
    settle(ctx, app);
}

pub fn key(k: Key, modifiers: Modifiers) -> Vec<Event> {
    vec![
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers },
        Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers },
    ]
}

pub fn settle(ctx: &egui::Context, app: &mut SoundApp) {
    for _ in 0..3 {
        frame(ctx, app, vec![]);
    }
}

pub fn click_mod(ctx: &egui::Context, app: &mut SoundApp, at: Pos2, m: Modifiers) {
    frame_m(ctx, app, vec![Event::PointerMoved(at)], m);
    frame_m(ctx, app, vec![button(at, true, m)], m);
    frame_m(ctx, app, vec![button(at, false, m)], m);
    settle(ctx, app);
}

pub fn click(ctx: &egui::Context, app: &mut SoundApp, at: Pos2) {
    click_mod(ctx, app, at, Modifiers::NONE);
}

pub fn drag(ctx: &egui::Context, app: &mut SoundApp, from: Pos2, to: Pos2, steps: usize) {
    frame(ctx, app, vec![Event::PointerMoved(from)]);
    frame(ctx, app, vec![button(from, true, Modifiers::NONE)]);
    for step in 1..=steps {
        frame(ctx, app, vec![Event::PointerMoved(from.lerp(to, step as f32 / steps as f32))]);
    }
    frame(ctx, app, vec![button(to, false, Modifiers::NONE)]);
    settle(ctx, app);
}

pub fn control(ctx: &egui::Context, name: &str) -> Option<Rect> {
    ctx.data(|d| d.get_temp::<Rect>(Id::new(format!("soundcraft-vocal-{name}"))))
}

pub fn text(ctx: &egui::Context, name: &str) -> Option<String> {
    ctx.data(|d| d.get_temp::<String>(Id::new(format!("soundcraft-vocal-{name}"))))
}

/// Every note of the track, in time order.
pub fn notes(app: &SoundApp, t: TrackId) -> Vec<NoteRef> {
    soundcraft_engine::vocal::track_notes(app.engine.session(), t, None)
}

/// A note's span in session samples.
pub fn span(app: &SoundApp, r: NoteRef) -> (Samples, Samples) {
    let s = app.engine.session();
    let (_, c) = s.find_clip(r.clip).unwrap();
    let a = soundcraft_engine::vocal::analysis_for(s, c).unwrap();
    let n = a.notes.iter().find(|n| n.id == r.note).unwrap();
    let hop = i64::from(a.track.hop_samples);
    let zero = c.start - c.source_offset();
    (zero + i64::from(n.start) * hop, zero + i64::from(n.end) * hop)
}

/// The note bars drawn last frame.
pub fn shapes(ctx: &egui::Context) -> Vec<soundcraft_ui_egui::vocal_editor::layout::NoteShape> {
    ctx.data(|d| d.get_temp(Id::new("soundcraft-vocal-shapes"))).unwrap_or_default()
}

/// The bar of note `r` drawn last frame.
pub fn bar(ctx: &egui::Context, r: NoteRef) -> Rect {
    shapes(ctx).into_iter().find(|s| s.r == r).map(|s| s.rect).unwrap_or_else(|| panic!("no bar for {r:?}"))
}

/// The roll's view last frame.
pub fn view(ctx: &egui::Context) -> soundcraft_ui_egui::vocal_editor::layout::RollView {
    ctx.data(|d| d.get_temp(Id::new("soundcraft-vocal-view"))).expect("the roll was drawn")
}

/// Low white noise (a breath) for `secs` at `db` dBFS RMS.
pub fn breath(secs: f64, db: f64) -> Vec<f32> {
    let n = (secs * f64::from(RATE)) as usize;
    let mut x: u32 = 0x1234_5678;
    let g = 10f64.powf(db / 20.0) * 3f64.sqrt();
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            ((f64::from(x) / f64::from(u32::MAX) * 2.0 - 1.0) * g) as f32
        })
        .collect()
}

/// Two notes with a 0.25 s breath between them.
pub fn take_breath() -> Vec<f32> {
    let mut x = vowel(cents(220.0, -30.0), 0.5, -21.0);
    x.extend(breath(0.25, -38.0));
    x.extend(vowel(cents(247.0, 20.0), 0.5, -21.0));
    x
}

/// Gives note `r` a scoop over the `frames` before it, in the cached analysis.
pub fn add_scoop(app: &mut SoundApp, r: NoteRef, frames: u32) {
    let s = app.engine.session_mut();
    let mut map = (*s.vocal.analyses).clone();
    for ca in map.values_mut() {
        let mut c = (**ca).clone();
        for n in c.analysis.notes.iter_mut().filter(|n| n.id == r.note) {
            n.scoop = Some((n.start.saturating_sub(frames), n.start));
        }
        *ca = std::sync::Arc::new(c);
    }
    s.vocal.analyses = std::sync::Arc::new(map);
}

/// The path of the split whose second side holds the Vocal Editor.
fn vocal_split(node: &craft_ui::docking::Node<soundcraft_ui_egui::docking::Panel>, path: &mut Vec<bool>) -> bool {
    use craft_ui::docking::Node;
    let Node::Split { first, second, .. } = node else { return false };
    if matches!(&**second, Node::Tabs { panels, .. } if panels.contains(&soundcraft_ui_egui::docking::Panel::Vocal)) {
        return true;
    }
    for (side, child) in [(false, first), (true, second)] {
        path.push(side);
        if vocal_split(child, path) {
            return true;
        }
        path.pop();
    }
    false
}
