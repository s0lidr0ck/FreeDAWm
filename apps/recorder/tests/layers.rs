//! Layers: an idea looping while another part records over it, each time round its own file,
//! every file starting at the idea's start and exactly the loop's length.

mod common;

use common::{BLOCK, Offline, RATE, offline, ramp, temp_dir};
use soundcraft_recorder::files::{self, TakeFiles};
use soundcraft_recorder::layers::{self, LayerRaw, Pending, Sidecar};
use soundcraft_recorder::rig::InputChoice;
use std::path::Path;
use std::time::{Duration, Instant};

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };
/// The idea: half a second.
const N: usize = 24_000;
/// A click in it, at this frame.
const CLICK_AT: usize = 1000;

fn write_wav(path: &Path, ch: Vec<Vec<f32>>) {
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: RATE, channels: ch };
    std::fs::write(path, soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).unwrap()).unwrap();
}

fn read_wav(path: &Path) -> Vec<Vec<f32>> {
    soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), Some("wav")).unwrap().1.channels
}

/// An idea `Idea.wav` in `dir`: silence but for a click at [`CLICK_AT`].
fn click_idea(dir: &Path) -> TakeFiles {
    let mut x = vec![0.0f32; N];
    x[CLICK_AT] = 0.8;
    write_wav(&dir.join("Idea.wav"), vec![x]);
    files::recent(dir, 10).remove(0)
}

/// A device where the output comes back into the input `delay` frames later (a cable, or the
/// speaker heard by the microphone), plus `own(i)`: what is played into the input.
struct Loopback {
    o: Offline,
    heard: Vec<f32>,
    delay: usize,
    next: Instant,
}

impl Loopback {
    fn new(o: Offline, delay: usize) -> Loopback {
        assert!(delay >= BLOCK, "the loop comes back after the block that played it");
        Loopback { o, heard: Vec::new(), delay, next: Instant::now() }
    }

    fn block(&mut self, own: &dyn Fn(usize) -> f32, bleed: f32) {
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now);
        }
        self.next = self.next.max(now) + Duration::from_secs_f64(BLOCK as f64 / f64::from(RATE));
        let fed = self.o.fed;
        let back = |i: usize| i.checked_sub(self.delay).and_then(|k| self.heard.get(k)).copied().unwrap_or(0.0);
        let v: Vec<f32> = (0..BLOCK).map(|f| own(fed + f) + bleed * back(fed + f)).collect();
        if let Some(i) = &self.o.input {
            i.push(&v);
        }
        self.o.fed += BLOCK;
        let mut out = vec![0.0f32; BLOCK * 2];
        self.o.audio.fill(&mut out, 2);
        self.heard.extend(out.chunks(2).map(|f| f[0]));
    }

    fn run(&mut self, secs: f64, own: &dyn Fn(usize) -> f32, bleed: f32) {
        let n = (secs * f64::from(RATE) / BLOCK as f64).ceil() as usize;
        for k in 0..n {
            self.block(own, bleed);
            if k % 4 == 0
                && let Some(s) = self.o.rig.step()
            {
                panic!("the take stopped by itself: {s:?}");
            }
        }
    }

    /// Line up: the click goes round the loop and is measured.
    fn line_up(&mut self) {
        // The device runs a moment first (it says what input it has as it runs).
        self.run(0.05, &silent, 1.0);
        self.o.rig.player.measure_latency(0, 0).unwrap();
        let end = Instant::now() + Duration::from_secs(8);
        while Instant::now() < end {
            self.block(&|_| 0.0, 1.0);
            self.o.rig.step();
            if let Some(r) = self.o.rig.player.measure_result() {
                let frames = r.unwrap();
                self.o.rig.measured = Some((frames, self.o.rig.player.measure_feed()));
                return;
            }
        }
        panic!("the measurement never came back");
    }
}

fn silent(_: usize) -> f32 {
    0.0
}

fn layer(raw: Option<LayerRaw>) -> layers::LayerResult {
    layers::finish(&raw.expect("a layer take"))
}

#[test]
fn a_layer_is_the_ideas_length_and_lines_up_with_what_was_heard() {
    let dir = temp_dir("layer-align");
    let idea = click_idea(&dir);
    let before = std::fs::read(dir.join("Idea.wav")).unwrap();
    // A round trip of 437 frames (9 ms): what the click re-recorded through it must land on the
    // idea's click once lined up.
    let mut lb = Loopback::new(offline(1, &[], MONO), 437);
    lb.line_up();
    let (measured, _) = lb.o.rig.measured.unwrap();
    eprintln!("Line up measured {measured} frames (the loop is 437 plus the feed's own)");
    let mix = layers::mix(&idea, &Sidecar::default(), RATE).unwrap();
    assert_eq!(mix.frames, N);
    lb.o.rig.start_layer(&dir, "Idea", idea.next_layer(), mix.channels, N as i64).unwrap();
    assert_eq!(lb.o.rig.layering(), Some(("Idea", 2)));
    // Started a little late into the second time round: the first is whole, the second padded.
    lb.run(0.5 * 1.5, &silent, 1.0);
    let res = layer(lb.o.rig.stop().layer);
    assert_eq!(res.problem, None);
    assert_eq!(res.made, vec![2, 3], "each time round its own layer");
    for n in [2, 3] {
        let ch = read_wav(&dir.join(format!("Idea (layer {n}).wav")));
        assert_eq!(ch[0].len(), N, "layer {n} is exactly the idea's length");
        let peak = (0..N).max_by(|a, b| ch[0][*a].abs().total_cmp(&ch[0][*b].abs())).unwrap();
        assert!(peak.abs_diff(CLICK_AT) <= 2, "layer {n}: the click re-recorded lands at {peak}, the idea's is at {CLICK_AT}");
    }
    let last = read_wav(&dir.join("Idea (layer 3).wav"));
    assert!(last[0][N - 2000..].iter().all(|x| *x == 0.0), "the part not played of the last time round is silence");
    assert_eq!(std::fs::read(dir.join("Idea.wav")).unwrap(), before, "the idea itself is never rewritten");
    let ideas = files::recent(&dir, 10);
    assert_eq!(ideas.len(), 1, "one idea, not three rows");
    assert_eq!(ideas[0].layer_count(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_line_up_the_reported_latency_and_the_nudge_place_it() {
    let dir = temp_dir("layer-nudge");
    let idea = click_idea(&dir);
    let mut lb = Loopback::new(offline(1, &[], MONO), 600);
    // What the devices report here (the test devices report the feed only), plus the nudge the
    // user set: together the true round trip.
    let reported = lb.o.rig.layer_offset();
    lb.o.rig.nudge = 600 - reported;
    let mix = layers::mix(&idea, &Sidecar::default(), RATE).unwrap();
    lb.o.rig.start_layer(&dir, "Idea", 2, mix.channels, N as i64).unwrap();
    lb.run(0.45, &silent, 1.0);
    let res = layer(lb.o.rig.stop().layer);
    assert_eq!(res.made, vec![2]);
    let ch = read_wav(&dir.join("Idea (layer 2).wav"));
    let peak = (0..N).max_by(|a, b| ch[0][*a].abs().total_cmp(&ch[0][*b].abs())).unwrap();
    assert!(peak.abs_diff(CLICK_AT) <= 2, "the click at {peak}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_new_layer_holds_only_the_input_never_the_playback() {
    let dir = temp_dir("layer-own");
    let idea = click_idea(&dir);
    let mut lb = Loopback::new(offline(1, &[], MONO), 300);
    let mix = layers::mix(&idea, &Sidecar::default(), RATE).unwrap();
    lb.o.rig.start_layer(&dir, "Idea", 2, mix.channels, N as i64).unwrap();
    // Headphones: nothing of the idea comes back, only what is played.
    let own = |i: usize| 0.1 * ramp(i, 0);
    lb.run(0.45, &own, 0.0);
    let res = layer(lb.o.rig.stop().layer);
    assert_eq!(res.made, vec![2]);
    let ch = read_wav(&dir.join("Idea (layer 2).wav"));
    let peak = ch[0].iter().fold(0f32, |m, x| m.max(x.abs()));
    assert!(peak <= 0.051, "the click (0.8) is not in it: {peak}");
    assert!(peak > 0.01, "what was played is");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_silent_time_round_is_not_kept_and_a_partial_one_is_padded() {
    let dir = temp_dir("layer-passes");
    let idea = click_idea(&dir);
    let mut lb = Loopback::new(offline(1, &[], MONO), 300);
    let mix = layers::mix(&idea, &Sidecar::default(), RATE).unwrap();
    lb.o.rig.start_layer(&dir, "Idea", idea.next_layer(), mix.channels, N as i64).unwrap();
    // Round 1 played, round 2 silent, round 3 played for a third of it.
    let own = |i: usize| if i % (3 * N) < N - 2000 || (i % (3 * N) >= 2 * N + 4000) { 0.2 * ramp(i, 0) } else { 0.0 };
    let start = lb.o.fed;
    let own = move |i: usize| own(i - start);
    lb.run(0.5 * 2.35, &own, 0.0);
    let res = layer(lb.o.rig.stop().layer);
    assert_eq!(res.empty, 1, "the silent time round");
    assert_eq!(res.made, vec![2, 3], "numbered without a gap");
    let third = read_wav(&dir.join("Idea (layer 3).wav"));
    assert_eq!(third[0].len(), N, "padded to the loop's length");
    assert!(third[0][N - 1000..].iter().all(|x| *x == 0.0), "with silence");
    assert!(!dir.join("Idea (layer 4).wav").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn notes_played_over_the_loop_make_midi_layers_in_time() {
    let dir = temp_dir("layer-midi");
    let idea = click_idea(&dir);
    let mut o = offline(1, &["Keys"], MONO);
    let mix = layers::mix(&idea, &Sidecar::default(), RATE).unwrap();
    o.rig.start_layer(&dir, "Idea", 2, mix.channels, N as i64).unwrap();
    o.run(0.2, &|_, _| 0.0);
    o.keys.send("Keys", &[0x90, 60, 100]);
    o.run(0.1, &|_, _| 0.0);
    o.keys.send("Keys", &[0x80, 60, 0]);
    // The second time round: another note, about 0.1 s in.
    o.run(0.3, &|_, _| 0.0);
    o.keys.send("Keys", &[0x90, 64, 100]);
    o.run(0.1, &|_, _| 0.0);
    o.keys.send("Keys", &[0x80, 64, 0]);
    o.run(0.1, &|_, _| 0.0);
    let res = layer(o.rig.stop().layer);
    assert_eq!(res.made, vec![2, 3], "{res:?}");
    assert!(!dir.join("Idea (layer 2).wav").exists(), "nothing sounded: no WAV, only the notes");
    let notes = |n: u32| {
        let smf = soundcraft_midi::read_smf(&std::fs::read(dir.join(format!("Idea (layer {n}).mid"))).unwrap()).unwrap();
        smf.tracks.iter().flat_map(|t| t.sequence.notes.clone()).collect::<Vec<_>>()
    };
    let (a, b) = (notes(2), notes(3));
    assert_eq!(a.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60]);
    assert_eq!(b.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![64]);
    // 120 bpm, 960 a beat: 1920 ticks a second, from the idea's start.
    let at = |n: &soundcraft_midi::Note| n.start as f64 / 1920.0;
    assert!((0.15..0.3).contains(&at(&a[0])), "the first note {} s into the idea", at(&a[0]));
    assert!((0.05..0.2).contains(&at(&b[0])), "the second {} s into its time round", at(&b[0]));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn layers_are_named_renamed_and_thrown_away_together_or_one_by_one() {
    let dir = temp_dir("layer-files");
    write_wav(&dir.join("Chorus idea.wav"), vec![vec![0.1; 4800]]);
    write_wav(&dir.join("Chorus idea (layer 2).wav"), vec![vec![0.1; 4800]]);
    std::fs::write(dir.join("Chorus idea (layer 2).mid"), b"MThd").unwrap();
    // Added by hand, with a gap: picked up as a layer.
    write_wav(&dir.join("Chorus idea (layer 5).wav"), vec![vec![0.1; 4800]]);
    write_wav(&dir.join("Verse.wav"), vec![vec![0.1; 4800]]);
    let mut side = Sidecar::default();
    side.layers.insert(2, layers::LayerMix { muted: true, level: 1.0 });
    side.save(&dir.join("Chorus idea.fdidea")).unwrap();
    let list = files::recent(&dir, 10);
    assert_eq!(list.len(), 2, "two ideas: {:?}", list.iter().map(|t| &t.stem).collect::<Vec<_>>());
    let chorus = list.iter().find(|t| t.stem == "Chorus idea").unwrap().clone();
    assert_eq!(chorus.all_layers().iter().map(|l| l.n).collect::<Vec<_>>(), vec![1, 2, 5]);
    assert!(chorus.layers[0].mid.is_some());
    assert_eq!(chorus.next_layer(), 6);
    assert!(chorus.sidecar.is_some());
    // Renaming renames every layer and the sidecar; a name another idea has is refused.
    assert!(files::rename(&chorus, "Verse").is_err());
    let renamed = files::rename(&chorus, "Hook").unwrap();
    for f in ["Hook.wav", "Hook (layer 2).wav", "Hook (layer 2).mid", "Hook (layer 5).wav", "Hook.fdidea"] {
        assert!(dir.join(f).exists(), "{f}");
    }
    assert!(!dir.join("Chorus idea (layer 2).wav").exists());
    assert!(Sidecar::of(&renamed).mix(2).muted, "what it remembers goes with it");
    // One layer away: the rest stay.
    let fewer = files::delete_layer(&renamed, 2).unwrap();
    assert!(!dir.join("Hook (layer 2).wav").exists() && !dir.join("Hook (layer 2).mid").exists());
    assert!(dir.join("Hook.wav").exists() && dir.join("Hook (layer 5).wav").exists());
    assert_eq!(fewer.layer_count(), 2);
    // All of it.
    files::delete(&fewer).unwrap();
    let left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left, vec!["Verse.wav".to_string()]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_mix_leaves_muted_layers_out_and_plays_each_at_its_level() {
    let dir = temp_dir("layer-mix");
    write_wav(&dir.join("A.wav"), vec![vec![0.1; 4800]]);
    write_wav(&dir.join("A (layer 2).wav"), vec![vec![0.2; 9600]]);
    let take = files::recent(&dir, 10).remove(0);
    let mut side = Sidecar::default();
    let m = layers::mix(&take, &side, RATE).unwrap();
    assert_eq!(m.frames, 9600, "the loop is the longest layer");
    assert!((m.channels[0][10] - 0.3).abs() < 1e-3 && (m.channels[1][5000] - 0.2).abs() < 1e-3);
    side.layers.insert(2, layers::LayerMix { muted: true, level: 1.0 });
    side.layers.insert(1, layers::LayerMix { muted: false, level: 0.5 });
    let m = layers::mix(&take, &side, RATE).unwrap();
    assert!((m.channels[0][10] - 0.05).abs() < 1e-3, "{}", m.channels[0][10]);
    assert_eq!(m.channels[0][5000], 0.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_crash_mid_layer_leaves_readable_files_finished_next_time() {
    let dir = temp_dir("layer-crash");
    let idea = click_idea(&dir);
    let before = std::fs::read(dir.join("Idea.wav")).unwrap();
    let mut o = offline(1, &[], MONO);
    let mix = layers::mix(&idea, &Sidecar::default(), RATE).unwrap();
    // The window notes the take beside the idea before it starts.
    let side = Sidecar { pending: Some(Pending { layer: 2, loop_frames: N as i64, rate: RATE, offset: 0 }), ..Sidecar::default() };
    side.save(&dir.join("Idea.fdidea")).unwrap();
    o.rig.start_layer(&dir, "Idea", 2, mix.channels, N as i64).unwrap();
    o.run(1.6, &|i, c| 0.3 * ramp(i, c));
    // The crash: what is on disk now is all there is.
    let after = temp_dir("layer-crash-after");
    for f in ["Idea.wav", "Idea (layer 2).wav", "Idea.fdidea"] {
        std::fs::copy(dir.join(f), after.join(f)).unwrap();
    }
    let raw = read_wav(&after.join("Idea (layer 2).wav"));
    assert!(raw[0].len() >= RATE as usize, "the take so far plays: {} frames", raw[0].len());
    assert_eq!(std::fs::read(after.join("Idea.wav")).unwrap(), before, "the earlier layer untouched");
    // Opened again: it is cut into its layers.
    let done = layers::recover(&after, None);
    assert_eq!(done.len(), 1);
    assert!(done[0].1.made.len() >= 3, "{:?}", done[0].1);
    for n in &done[0].1.made {
        assert_eq!(read_wav(&after.join(format!("Idea (layer {n}).wav")))[0].len(), N);
    }
    assert_eq!(Sidecar::load(&after.join("Idea.fdidea")).pending, None, "done: not again");
    assert!(layers::recover(&after, None).is_empty());
    o.rig.stop();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(after);
}
