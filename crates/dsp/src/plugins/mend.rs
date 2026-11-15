//! Mend: cleans a voice as it plays. It takes the room, the noise and the clipping off a vocal
//! with the Smule Renaissance model (the one Clean Vocal uses), from an insert on the track.
//!
//! - The model does not run here. It needs most of a second of sound after each moment to judge
//!   it, and far too much arithmetic for the audio thread. Mend sends its input to a worker
//!   through a [`link::Link`], the worker runs the model a block at a time and sends the cleaned
//!   sound back, and Mend plays it against the input delayed by the same amount. The worker
//!   belongs to the desktop app (`soundcraft_ml::live`); a build without it (the browser's)
//!   plays the input through.
//! - **Latency: about 2.9 s, always** ([`link::Plan::latency`]): one block, the model's
//!   look-ahead, and one more block for the model to answer in. Delay compensation hides it on
//!   playback. It is no use while recording: monitor without it.
//! - When the answer is not there in time (the model is not installed, still loading, or the
//!   computer cannot keep up), Mend plays the delayed input for that stretch. An offline render
//!   ([`Plugin::set_offline`]) waits for the answer instead, so a bounce is always cleaned.
//! - Mono and stereo. A stereo strip carrying the same sound on both sides costs the model one
//!   pass, two different sides cost two.
//!
//! `readouts`: `[0]` the worker's [`link::State`] as a number, `[1]` the model's time per block
//! as a percentage of the block's length (100: only just keeping up), `[2]` 1 while the cleaned
//! sound is playing (0: the input is), `[3]` the latency in seconds, `[4]` how many times the
//! answer has been late since `prepare`.

pub mod link;

use crate::params::{Params, choice, lin};
use crate::util::{frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};
use link::{Link, Plan, State};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ID: &str = "mend";

const P_AMOUNT: usize = 0;
const P_LISTEN: usize = 1;
const P_OUT: usize = 2;

/// How long the swap between the cleaned sound and the input takes, in ms.
const SWAP_MS: f32 = 8.0;
/// How long the controls take to reach a new value, in ms.
const GLIDE_MS: f32 = 20.0;
/// How long an offline render waits for an answer that makes no progress before it plays the
/// input instead.
const OFFLINE_PATIENCE: Duration = Duration::from_secs(20);
/// How long it waits for a worker that exists to pick the link up.
const PICKUP_PATIENCE: Duration = Duration::from_secs(5);
/// The same while the model is still loading.
const LOADING_PATIENCE: Duration = Duration::from_secs(90);

static MEND_PARAMS: [ParamInfo; 3] = [
    lin("amount", "Amount", 0.0, 100.0, 100.0, Unit::Percent),
    choice("listen", "Listen", &["Cleaned", "Removed"], 0),
    lin("out", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

pub static MEND_INFO: PluginInfo = PluginInfo {
    id: ID,
    name: "Mend",
    short_name: "Mend",
    category: Category::Other,
    params: &MEND_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// Links no worker has picked up yet. The worker takes them ([`take_new_links`]); `prepare` adds
/// one per instance.
static NEW_LINKS: Mutex<Vec<Arc<Link>>> = Mutex::new(Vec::new());
/// The most links kept waiting for a worker (a build without one never takes them).
const MAX_WAITING: usize = 64;

/// Whether this process has a worker ([`set_worker_present`]).
static WORKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Says a worker is serving this process's links: an offline render then waits for it to pick
/// its link up instead of taking a link nobody has answered yet for one nobody will.
pub fn set_worker_present() {
    WORKER.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Every link made since the last call, for the worker that will serve them.
pub fn take_new_links() -> Vec<Arc<Link>> {
    let mut links = NEW_LINKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    std::mem::take(&mut *links)
}

fn offer(link: &Arc<Link>) {
    let mut links = NEW_LINKS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    links.retain(|l| !l.closed());
    if links.len() < MAX_WAITING {
        links.push(Arc::clone(link));
    }
}

pub struct Mend {
    p: Params,
    sr: f32,
    ch: usize,
    link: Option<Arc<Link>>,
    offline: bool,
    latency: usize,
    /// The input, kept for one latency: `[channel][frame]`, written round and round.
    dry: Vec<Vec<f32>>,
    dry_at: usize,
    /// Answer frames for one chunk, interleaved.
    wet: Vec<f32>,
    /// Frames sent on every stream so far, and frames run on this one.
    sent: u64,
    run: u64,
    stream: u32,
    /// Answer frames taken or dropped so far, on every stream.
    taken: u64,
    /// 1 while the cleaned sound plays, 0 while the input does, gliding between.
    have: f32,
    swap_coef: f32,
    amount: f32,
    gain: f32,
    glide_coef: f32,
    amount_to: f32,
    gain_to: f32,
    removed: bool,
    late: u32,
    was_late: bool,
}

impl Default for Mend {
    fn default() -> Self {
        Self::new()
    }
}

fn coef(ms: f32, sr: f32) -> f32 {
    1.0 - (-1.0 / (ms * 0.001 * sr).max(1.0)).exp()
}

impl Mend {
    pub fn new() -> Self {
        let mut m = Mend {
            p: Params::new(&MEND_PARAMS),
            sr: 48_000.0,
            ch: 2,
            link: None,
            offline: false,
            latency: 0,
            dry: Vec::new(),
            dry_at: 0,
            wet: Vec::new(),
            sent: 0,
            run: 0,
            stream: 0,
            taken: 0,
            have: 0.0,
            swap_coef: 1.0,
            amount: 1.0,
            gain: 1.0,
            glide_coef: 1.0,
            amount_to: 1.0,
            gain_to: 1.0,
            removed: false,
            late: 0,
            was_late: false,
        };
        m.update();
        m
    }

    fn update(&mut self) {
        self.amount_to = (self.p.v(P_AMOUNT) / 100.0).clamp(0.0, 1.0);
        self.gain_to = db_to_gain(self.p.v(P_OUT));
        self.removed = self.p.choice(P_LISTEN) == 1;
    }

    /// The link to the worker, for tests and for a host that serves it itself.
    pub fn link(&self) -> Option<&Arc<Link>> {
        self.link.as_ref()
    }

    /// Begins a new stream at the next frame sent: what was on its way is dropped.
    fn restart(&mut self) {
        self.stream = self.stream.wrapping_add(1);
        self.run = 0;
        if let Some(link) = &self.link {
            link.restart(self.stream, self.sent);
        }
    }

    /// Waits (offline only) until `need` answer frames of this stream are there, or the worker
    /// plainly will not give them.
    fn wait_for(link: &Link, stream: u32, need: usize, ready: impl Fn(&Link) -> usize) {
        let mut since = Instant::now();
        let mut last = usize::MAX;
        loop {
            let patience = match link.state() {
                State::Ready => OFFLINE_PATIENCE,
                State::Loading => LOADING_PATIENCE,
                State::Waiting if WORKER.load(std::sync::atomic::Ordering::Relaxed) => PICKUP_PATIENCE,
                State::Waiting | State::NoModel | State::Failed => return,
            };
            let got = if link.answer_from().0 == stream { ready(link) } else { 0 };
            if got >= need {
                return;
            }
            if got != last {
                (last, since) = (got, Instant::now());
            } else if since.elapsed() > patience {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// One chunk of at most `self.wet`'s frames.
    fn chunk(&mut self, io: &mut [Vec<f32>], start: usize, n: usize) {
        let Some(link) = self.link.clone() else { return };
        let ch = self.ch.min(io.len()).min(self.dry.len());
        let lat = self.latency as u64;
        if link.take_lost() {
            self.restart();
        }
        // Send the input. A full queue (the worker has stopped taking) starts the stream again
        // after this chunk, which plays the input.
        let fits = link.room_for(n);
        if fits {
            link.send(io, start, n);
            self.sent += n as u64;
        }
        // The answers this chunk plays: those of frames `run - latency` on.
        let need = (self.run + n as u64).saturating_sub(lat).min(n as u64) as usize;
        let skip = n - need;
        let want = (self.run + skip as u64).saturating_sub(lat);
        let mut got = 0;
        if fits && need > 0 {
            let stream = self.stream;
            let taken = self.taken;
            // Frames of this stream's answers waiting beyond what has to be dropped first.
            let ahead = move |l: &Link| -> usize {
                let (_, from) = l.answer_from();
                let end = taken + l.answers() as u64;
                end.saturating_sub((from + want).max(taken)).min(usize::MAX as u64) as usize
            };
            // (The browser build has no worker and no clock to wait by.)
            if self.offline && cfg!(not(target_arch = "wasm32")) {
                Self::wait_for(&link, stream, need, ahead);
            }
            let (s, from) = link.answer_from();
            if s == stream {
                // Drop what is before this stream's answers, and answers that came too late.
                let at = from + want;
                if at > self.taken {
                    self.taken += link.skip_answers((at - self.taken).min(usize::MAX as u64) as usize) as u64;
                }
                if self.taken == at {
                    got = link.take_answers(&mut self.wet, need);
                    self.taken += got as u64;
                }
            } else {
                // Answers of a stream this instance has left.
                self.taken += link.skip_answers(usize::MAX / 4) as u64;
            }
        }
        let late = need > got;
        if late && !self.was_late && link.state() == State::Ready {
            self.late = self.late.saturating_add(1);
        }
        self.was_late = late;
        let wch = link.channels;
        let len = self.dry.first().map_or(0, Vec::len).max(1);
        for i in 0..n {
            let wet_i = i.checked_sub(skip).filter(|k| *k < got);
            let target = if wet_i.is_some() { 1.0 } else { 0.0 };
            self.have += (target - self.have) * self.swap_coef;
            self.amount += (self.amount_to - self.amount) * self.glide_coef;
            self.gain += (self.gain_to - self.gain) * self.glide_coef;
            let at = (self.dry_at + i) % len;
            let k = self.amount * self.have;
            for c in 0..ch {
                let x = io.get(c).and_then(|b| b.get(start + i)).copied().unwrap_or(0.0);
                let d = self.dry.get(c).and_then(|b| b.get(at)).copied().unwrap_or(0.0);
                let w = wet_i.and_then(|j| self.wet.get(j * wch + c.min(wch - 1))).copied().unwrap_or(d);
                let y = if self.removed { k * (d - w) } else { d + k * (w - d) };
                if let Some(o) = io.get_mut(c).and_then(|b| b.get_mut(start + i)) {
                    *o = if y.is_finite() { y * self.gain } else { 0.0 };
                }
                if let Some(slot) = self.dry.get_mut(c).and_then(|b| b.get_mut(at)) {
                    *slot = x;
                }
            }
        }
        self.dry_at = (self.dry_at + n) % len;
        self.run += n as u64;
        if !fits {
            self.restart();
        }
    }
}

impl Drop for Mend {
    fn drop(&mut self) {
        if let Some(link) = &self.link {
            link.close();
        }
    }
}

impl Plugin for Mend {
    crate::params::param_plumbing!(MEND_INFO);

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels).min(2);
        if let Some(old) = self.link.take() {
            old.close();
        }
        self.swap_coef = coef(SWAP_MS, self.sr);
        self.glide_coef = coef(GLIDE_MS, self.sr);
        let plan = Plan::for_rate(self.sr.round() as u32);
        self.latency = plan.map_or(0, |p| p.latency());
        self.dry = (0..self.ch).map(|_| vec![0.0; self.latency.max(1)]).collect();
        self.wet = vec![0.0; max_block.clamp(64, 8192) * self.ch];
        self.link = plan.map(|p| {
            let link = Arc::new(Link::new(p, self.ch));
            link.set_offline(self.offline);
            offer(&link);
            link
        });
        (self.sent, self.taken, self.stream, self.late) = (0, 0, 0, 0);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        for d in &mut self.dry {
            d.fill(0.0);
        }
        self.dry_at = 0;
        self.have = 0.0;
        self.amount = self.amount_to;
        self.gain = self.gain_to;
        self.was_late = false;
        self.restart();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let frames = frames_in(io, frames, self.ch);
        let step = (self.wet.len() / self.ch.max(1)).max(1);
        let mut start = 0;
        while start < frames {
            let n = step.min(frames - start);
            self.chunk(io, start, n);
            start += n;
        }
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
        if let Some(link) = &self.link {
            link.set_offline(offline);
        }
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let (state, load) = self.link.as_ref().map_or((State::Waiting, 0.0), |l| (l.state(), l.load()));
        out[0] = f32::from(state as u8);
        out[1] = load * 100.0;
        out[2] = if self.have > 0.5 { 1.0 } else { 0.0 };
        out[3] = self.latency as f32 / self.sr;
        out[4] = self.late as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// A stand-in worker: answers every block of `link` with the input times `gain`, as the real
    /// one does (a block is answered once it and its look-ahead are in).
    struct Echo {
        link: Arc<Link>,
        heard: Vec<f32>,
        stream: u32,
        from: u64,
        received: u64,
        block: u64,
        made: u64,
        gain: f32,
    }

    impl Echo {
        fn new(link: &Arc<Link>, gain: f32) -> Echo {
            link.set_state(State::Ready);
            Echo { link: Arc::clone(link), heard: Vec::new(), stream: 0, from: 0, received: 0, block: 0, made: 0, gain }
        }

        fn step(&mut self) {
            let ch = self.link.channels;
            let mut fresh = Vec::new();
            let n = self.link.receive(&mut fresh) as u64;
            let (s, from) = self.link.stream();
            if s != self.stream {
                (self.stream, self.from, self.block) = (s, from, 0);
                self.heard.clear();
                self.link.answer_starts(s, self.made);
            }
            // Keep the frames of this stream only.
            let first = self.received;
            self.received += n;
            for (i, f) in fresh.chunks(ch).enumerate() {
                if first + i as u64 >= self.from {
                    self.heard.extend_from_slice(f);
                }
            }
            let p = self.link.plan;
            loop {
                let end = (self.block as usize + 1) * p.block;
                if self.heard.len() / ch < end + p.look || !self.link.room_for_answer(p.block) {
                    return;
                }
                let out: Vec<f32> = self.heard[(end - p.block) * ch..end * ch].iter().map(|v| v * self.gain).collect();
                self.link.answer(&out);
                self.made += p.block as u64;
                self.block += 1;
            }
        }
    }

    fn prepared() -> Mend {
        let mut m = Mend::new();
        m.prepare(SR, 512, 2);
        m
    }

    fn tone(n: usize, from: usize) -> Vec<f32> {
        (from..from + n).map(|i| (i as f32 * 0.01).sin() * 0.5).collect()
    }

    /// Runs `total` frames of a tone through `m` in blocks of 512, stepping the stand-in worker
    /// between blocks; the left output.
    fn play(m: &mut Mend, echo: &mut Option<Echo>, total: usize, from: usize) -> Vec<f32> {
        let x = tone(total, from);
        let mut out = Vec::with_capacity(total);
        for b in x.chunks(512) {
            let mut io = vec![b.to_vec(), b.to_vec()];
            m.process(&mut io, b.len());
            out.extend_from_slice(&io[0]);
            if let Some(e) = echo {
                e.step();
            }
        }
        out
    }

    #[test]
    fn without_a_worker_the_input_comes_out_one_latency_later() {
        let mut m = prepared();
        let lat = m.latency();
        assert_eq!(lat, 137_216);
        let total = lat + 20_000;
        let y = play(&mut m, &mut None, total, 0);
        let x = tone(total, 0);
        assert!(y[..lat].iter().all(|v| *v == 0.0));
        for i in 0..20_000 {
            assert!((y[lat + i] - x[i]).abs() < 1e-6, "frame {i}");
        }
        let mut r = [f32::NAN; 8];
        m.readouts(&mut r);
        assert_eq!((r[0], r[2], r[4]), (0.0, 0.0, 0.0));
        assert!((r[3] - lat as f32 / SR).abs() < 1e-4);
    }

    #[test]
    fn the_answer_plays_in_step_with_the_delayed_input() {
        let mut m = prepared();
        let lat = m.latency();
        let mut echo = Some(Echo::new(m.link().unwrap(), 0.5));
        let total = lat + 150_000;
        let y = play(&mut m, &mut echo, total, 0);
        let x = tone(total, 0);
        // After the swap has settled: the worker's half-level copy, exactly in time.
        for i in 4_000..150_000 {
            assert!((y[lat + i] - 0.5 * x[i]).abs() < 1e-4, "frame {i}: {} against {}", y[lat + i], 0.5 * x[i]);
        }
        let mut r = [f32::NAN; 8];
        m.readouts(&mut r);
        assert_eq!((r[0], r[2], r[4]), (2.0, 1.0, 0.0));
    }

    #[test]
    fn amount_listen_and_output_do_what_they_say() {
        let run = |set: &[(&str, f32)]| -> (Vec<f32>, Vec<f32>, usize) {
            let mut m = prepared();
            for (id, v) in set {
                assert!(m.set_param(id, *v));
            }
            m.reset();
            let lat = m.latency();
            let mut echo = Some(Echo::new(m.link().unwrap(), 0.5));
            let total = lat + 60_000;
            (play(&mut m, &mut echo, total, 0), tone(total, 0), lat)
        };
        let (y, x, lat) = run(&[("amount", 50.0)]);
        assert!((y[lat + 30_000] - 0.75 * x[30_000]).abs() < 1e-4);
        let (y, x, lat) = run(&[("listen", 1.0)]);
        assert!((y[lat + 30_000] - 0.5 * x[30_000]).abs() < 1e-4, "what was taken away");
        let (y, x, lat) = run(&[("amount", 0.0), ("out", -6.0)]);
        assert!((y[lat + 30_000] - db_to_gain(-6.0) * x[30_000]).abs() < 1e-4);
    }

    #[test]
    fn a_reset_starts_a_clean_stream() {
        let mut m = prepared();
        let lat = m.latency();
        let mut echo = Some(Echo::new(m.link().unwrap(), 0.5));
        play(&mut m, &mut echo, lat + 70_000, 0);
        m.reset();
        // Nothing of the old stream comes out after the reset, and the new one lines up.
        let total = lat + 120_000;
        let y = play(&mut m, &mut echo, total, 1_000_000);
        let x = tone(total, 1_000_000);
        assert!(y[..lat].iter().all(|v| *v == 0.0), "silence for one latency");
        for i in 4_000..120_000 {
            assert!((y[lat + i] - 0.5 * x[i]).abs() < 1e-4, "frame {i}");
        }
    }

    #[test]
    fn a_late_answer_plays_the_input_and_is_counted() {
        let mut m = prepared();
        let lat = m.latency();
        let mut echo = Echo::new(m.link().unwrap(), 0.5);
        let total = lat + 400_000;
        let x = tone(total, 0);
        let mut y = Vec::new();
        for (k, b) in x.chunks(512).enumerate() {
            let mut io = vec![b.to_vec(), b.to_vec()];
            m.process(&mut io, b.len());
            y.extend_from_slice(&io[0]);
            // The worker stalls for a while once the answers have begun.
            let at = k * 512;
            if !(lat + 20_000..lat + 220_000).contains(&at) {
                echo.step();
            }
        }
        // During the stall the input plays; after it the answer is back, still in time.
        let (mut dry, mut wet) = (0, 0);
        for i in 0..400_000 {
            let (d, w) = ((y[lat + i] - x[i]).abs(), (y[lat + i] - 0.5 * x[i]).abs());
            if x[i].abs() > 0.1 {
                dry += usize::from(d < 1e-4);
                wet += usize::from(w < 1e-4);
            }
        }
        assert!(dry > 50_000, "the input played through the stall ({dry})");
        assert!(wet > 50_000, "the answer played either side of it ({wet})");
        for i in 390_000..400_000 {
            assert!((y[lat + i] - 0.5 * x[i]).abs() < 1e-4, "frame {i}");
        }
        let mut r = [f32::NAN; 8];
        m.readouts(&mut r);
        assert!(r[4] >= 1.0);
    }

    #[test]
    fn an_offline_render_waits_for_the_answer() {
        let mut m = Mend::new();
        m.set_offline(true);
        m.prepare(SR, 512, 2);
        let lat = m.latency();
        let link = Arc::clone(m.link().unwrap());
        assert!(link.offline());
        // A worker on its own thread, slower than the render.
        let worker = std::thread::spawn({
            let link = Arc::clone(&link);
            move || {
                let mut echo = Echo::new(&link, 0.5);
                while !link.closed() {
                    std::thread::sleep(Duration::from_millis(3));
                    echo.step();
                }
            }
        });
        while link.state() != State::Ready {
            std::thread::yield_now();
        }
        let total = lat + 120_000;
        let y = play(&mut m, &mut None, total, 0);
        let x = tone(total, 0);
        for i in 4_000..120_000 {
            assert!((y[lat + i] - 0.5 * x[i]).abs() < 1e-4, "frame {i}");
        }
        drop(m);
        worker.join().unwrap();
    }

    #[test]
    fn prepare_offers_a_link_and_dropping_closes_it() {
        let m = prepared();
        let link = Arc::clone(m.link().unwrap());
        assert!(take_new_links().iter().any(|l| Arc::ptr_eq(l, &link)) || link.state() == State::Waiting);
        assert!(!link.closed());
        drop(m);
        assert!(link.closed());
    }

    #[test]
    fn an_odd_rate_passes_the_sound_through() {
        let mut m = Mend::new();
        m.prepare(44_101.0, 256, 2);
        assert_eq!(m.latency(), 0);
        let mut io = vec![vec![0.25f32; 256]; 2];
        m.process(&mut io, 256);
        assert!(io[0].iter().all(|v| *v == 0.25));
    }
}
