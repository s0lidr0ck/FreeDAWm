//! Audio input capture for recording.
//!
//! The input stream pushes each block into an [`InputFeed`] (lock-free, preallocated); the output
//! callback pops it into the mix and, while a take records, copies the same frames into the
//! [`Capture`] ring. So a take holds exactly what the mix heard, at the session rate, and its first
//! frame has an exact timeline position ([`Recorder::start_pos`]). A writer thread streams the ring
//! into one WAV file per take, rewriting the header every second so a crash leaves a playable file.
//!
//! The input stream is the player's own when input and output are one device (duplex, see
//! [`crate::duplex`] and [`crate::Player::duplex_recorder`]), else the [`Recorder`]'s
//! ([`Recorder::open_with`]).

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use soundcraft_model::TrackId;
use soundcraft_time::Samples;

use crate::feed::{InputFeed, SampleRing};

/// What [`Recorder::finish`] returns: the takes that hold audio and the first problem met.
pub type Takes = (Vec<(TrackId, PathBuf)>, Option<String>);

/// Frames each block of a [`LiveTake`] sums up.
pub const LIVE_BLOCK: usize = 128;
/// Blocks a live take keeps per channel at most (three hours at 48 kHz); its file records on.
const LIVE_BLOCKS: usize = 1 << 22;

/// A take while it records, summed up for drawing: per channel, the (min, max) of each
/// [`LIVE_BLOCK`] frames so far.
pub type LiveTake = Vec<Vec<(f32, f32)>>;

const RING_SECONDS: usize = 4;
/// How long a recording input may go without a callback before it counts as gone.
const STALL: Duration = Duration::from_secs(2);
const GONE: u8 = 1;
const BROKEN: u8 = 2;

/// Why the input can't go on.
#[derive(Debug, Clone, PartialEq)]
pub enum Trouble {
    /// The device went away or stopped sending audio, or its callback hit a bug.
    Device(String),
    /// Writing a take failed (a full disk, say).
    Disk(String),
}

/// What the output callback, the input stream, the writer thread and the [`Recorder`] share.
pub(crate) struct Capture {
    pub(crate) armed: AtomicBool,
    /// Interleaved frames of every input channel, at the session rate (output callback → writer).
    pub(crate) ring: SampleRing,
    /// Samples lost while the ring was full.
    dropped: AtomicUsize,
    /// Input callbacks so far, to tell a device that stopped sending.
    beats: AtomicU64,
    /// Set from the input stream when it can't go on: `GONE` or `BROKEN`.
    fault: AtomicU8,
    /// The first failure writing a take, until it's reported.
    failed: Mutex<Option<String>>,
    /// The timeline position of the take's first frame, once `started`.
    pub(crate) start_pos: AtomicI64,
    /// The input feed's latency when the take's first frame was captured, once `started`.
    pub(crate) start_feed: AtomicU32,
    /// The mix's own delay (PDC, frames) after the take's first block rendered, once `started`.
    pub(crate) start_mix: AtomicU32,
    pub(crate) started: AtomicBool,
}

impl Capture {
    /// A capture with room for a few seconds of `channels` at `rate`.
    pub(crate) fn for_rate(rate: u32, channels: usize) -> Arc<Capture> {
        Arc::new(Capture::new(SampleRing::new((rate as usize).saturating_mul(channels.max(1) * RING_SECONDS).min(1 << 25))))
    }

    fn new(ring: SampleRing) -> Capture {
        let (beats, fault, failed) = (AtomicU64::new(0), AtomicU8::new(0), Mutex::new(None));
        Capture {
            armed: AtomicBool::new(false),
            ring,
            dropped: AtomicUsize::new(0),
            beats,
            fault,
            failed,
            start_pos: AtomicI64::new(0),
            start_feed: AtomicU32::new(0),
            start_mix: AtomicU32::new(0),
            started: AtomicBool::new(false),
        }
    }

    /// Run the input callback's `work`. After a panic in it, the input is broken and does
    /// nothing more.
    fn guarded(&self, work: impl FnOnce()) {
        self.beats.fetch_add(1, Ordering::Relaxed);
        if self.fault.load(Ordering::Relaxed) != BROKEN && std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
            self.fault.store(BROKEN, Ordering::Relaxed);
        }
    }

    /// Queue `len` samples (`sample(i)` is the i-th) of a recording block. Samples that don't fit
    /// are owed (`owed`) and queued as silence before the next block, so the take keeps its
    /// timing. The output callback's work; it never allocates.
    pub(crate) fn capture_with(&self, len: usize, sample: impl Fn(usize) -> f32, owed: &mut usize) {
        *owed -= self.ring.push(*owed, |_| 0.0);
        let kept = if *owed == 0 { self.ring.push(len, |i| crate::feed::finite(sample(i))) } else { 0 };
        *owed = owed.saturating_add(len - kept);
        self.dropped.fetch_add(len - kept, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn capture(&self, data: &[f32], owed: &mut usize) {
        self.capture_with(data.len(), |i| data.get(i).copied().unwrap_or(0.0), owed);
    }
}

pub struct Recorder {
    capture: Arc<Capture>,
    /// The input stream when the recorder owns it (split); a duplex input is the player's.
    #[cfg(not(target_os = "freebsd"))]
    _stream: Option<cpal::Stream>,
    pub device_name: String,
    /// The device's channel names, in channel order, when it gives them (JACK's ports); empty
    /// otherwise.
    pub channel_names: Vec<String>,
    /// The input device's rate.
    pub device_rate: u32,
    /// The rate takes are written at: the session's (they are captured from the mix's input).
    pub sample_rate: u32,
    pub channels: usize,
    /// The take writer, and what each track's take holds so far.
    writer: Option<(std::thread::JoinHandle<Takes>, Vec<(TrackId, Arc<Mutex<LiveTake>>)>)>,
    /// The callback count last seen, and since when.
    seen: (u64, Instant),
}

/// The input stream: device samples are converted to f32 in a scratch buffer allocated here (so
/// the callback never allocates), then pushed into the feed. It does not capture: the output
/// callback does, from the frames the mix sees.
#[cfg(not(target_os = "freebsd"))]
fn build_input<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    capture: &Arc<Capture>,
    feed: &Arc<InputFeed>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::Sample;
    use cpal::traits::DeviceTrait;
    let (sh, errors) = (Arc::clone(capture), Arc::clone(capture));
    let feed = Arc::clone(feed);
    let frame = usize::from(config.channels).max(1);
    let mut scratch = vec![0.0f32; 8192 * frame];
    let chunk = scratch.len();
    crate::guarded_build(move || {
        device.build_input_stream(
            *config,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                crate::mark_audio_thread();
                // The reported input latency: from capture until this callback.
                let at = info.timestamp();
                if let Some(d) = at.callback.checked_duration_since(at.capture) {
                    feed.set_input_latency(d);
                }
                sh.guarded(|| {
                    for part in data.chunks(chunk) {
                        let Some(s) = scratch.get_mut(..part.len()) else { return };
                        for (o, x) in s.iter_mut().zip(part) {
                            *o = f32::from_sample(*x);
                        }
                        feed.push(s);
                    }
                });
            },
            move |e| {
                // Possibly the audio thread (see `crate::mark_audio_thread`).
                crate::mark_audio_thread();
                if e.kind() == cpal::ErrorKind::DeviceNotAvailable {
                    let _ = errors.fault.compare_exchange(0, GONE, Ordering::Relaxed, Ordering::Relaxed);
                }
                log::warn!("input stream error: {e}");
            },
            None,
        )
    })
}

/// An input stream, built but not started, and what it feeds.
pub(crate) struct InputStream {
    #[cfg(not(target_os = "freebsd"))]
    pub(crate) stream: cpal::Stream,
    pub(crate) feed: Arc<InputFeed>,
    pub(crate) capture: Arc<Capture>,
    pub(crate) name: String,
    pub(crate) rate: u32,
    pub(crate) channels: usize,
}

/// Build an input stream on `device` at `rate` when the device offers it (else its default), with
/// `buffer` frames when it allows (else its default). It feeds `make_feed(channels, device
/// rate)`, and its capture holds frames at `session_rate`. Integer devices (ASIO drivers usually
/// are) are converted to f32.
#[cfg(not(target_os = "freebsd"))]
pub(crate) fn build_input_stream(
    device: &cpal::Device,
    rate: Option<u32>,
    buffer: Option<u32>,
    session_rate: u32,
    route: Option<&crate::jack::Route>,
    make_feed: impl Fn(usize, u32) -> Arc<InputFeed>,
) -> Result<InputStream, String> {
    use cpal::traits::DeviceTrait;
    // On JACK the stream goes by the device it is connected to (see `jack`).
    let name = route.map(|r| r.group.name.clone()).unwrap_or_else(|| device.description().map_or_else(|_| "Input".into(), |d| d.name().to_string()));
    // At the chosen rate when the device offers it, so input and output run together: the
    // best format first, then the most channels.
    let at_rate = rate.and_then(|r| {
        device
            .supported_input_configs()
            .ok()?
            .filter(|c| crate::devices::format_rank(c.sample_format()).is_some() && c.min_sample_rate() <= r && c.max_sample_rate() >= r)
            .min_by_key(|c| (crate::devices::format_rank(c.sample_format()), std::cmp::Reverse(c.channels())))
            .map(|c| c.with_sample_rate(r))
    });
    let cfg = match at_rate {
        Some(c) => c,
        None => device.default_input_config().map_err(|e| e.to_string())?,
    };
    let format = cfg.sample_format();
    if crate::devices::format_rank(format).is_none() {
        return Err(format!("unsupported input sample format {format:?}"));
    }
    let mut config = cfg.config();
    config.buffer_size = crate::devices::buffer_size(buffer, cfg.buffer_size());
    if let Some(r) = route {
        config.channels = r.channels();
    }
    let channels = usize::from(config.channels).max(1);
    let feed = make_feed(channels, config.sample_rate);
    let capture = Capture::for_rate(session_rate, channels);
    let build = |config: &cpal::StreamConfig| match format {
        cpal::SampleFormat::I32 => build_input::<i32>(device, config, &capture, &feed),
        cpal::SampleFormat::I16 => build_input::<i16>(device, config, &capture, &feed),
        _ => build_input::<f32>(device, config, &capture, &feed),
    };
    // The chosen buffer size first; if the device refuses it, its default.
    let stream = match build(&config) {
        Ok(s) => s,
        Err(e) if config.buffer_size != cpal::BufferSize::Default => {
            log::warn!("cannot open the input with buffer {:?}: {e}; using the device default", config.buffer_size);
            config.buffer_size = cpal::BufferSize::Default;
            build(&config).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    };
    Ok(InputStream { stream, feed, capture, name, rate: config.sample_rate, channels })
}

#[cfg(target_os = "freebsd")]
pub(crate) fn build_input_stream(
    _device: &cpal::Device,
    _rate: Option<u32>,
    _buffer: Option<u32>,
    _session_rate: u32,
    _route: Option<&crate::jack::Route>,
    _make_feed: impl Fn(usize, u32) -> Arc<InputFeed>,
) -> Result<InputStream, String> {
    Err("recording is not supported on this platform yet".into())
}

impl Recorder {
    /// A recorder over `capture`, whose input is `device_name` at `device_rate` with `channels`.
    pub(crate) fn attached(capture: Arc<Capture>, device_name: String, device_rate: u32, session_rate: u32, channels: usize) -> Recorder {
        Recorder {
            capture,
            #[cfg(not(target_os = "freebsd"))]
            _stream: None,
            device_name,
            channel_names: Vec::new(),
            device_rate,
            sample_rate: session_rate.max(1),
            channels: channels.max(1),
            writer: None,
            seen: (0, Instant::now()),
        }
    }

    /// Open the input device `audio` chooses (else the default) as its own stream (split: not the
    /// player's device), at the chosen rate and buffer size when the device offers them. Its
    /// input reaches `player` through a drift-correcting feed. Errors when there is no input
    /// (the caller shows a message).
    #[cfg(not(target_os = "freebsd"))]
    pub fn open_with(audio: &crate::AudioConfig, player: &crate::Player) -> Result<Recorder, String> {
        use cpal::traits::StreamTrait;
        let host = crate::devices::host(audio);
        let device = crate::devices::device(&host, audio.input.as_deref(), false).ok_or("no audio input device")?;
        let (session_rate, max_block) = (player.session_rate(), player.max_block);
        let route = crate::jack_input_route(&host, &device, audio.input.as_deref());
        let input = build_input_stream(&device, audio.sample_rate, audio.buffer_size, session_rate, route.as_ref(), |ch, rate| {
            InputFeed::drift(ch, rate, session_rate, max_block)
        })?;
        input.stream.play().map_err(|e| e.to_string())?;
        if let Some(r) = &route {
            r.connect();
        }
        player.attach_input(Arc::clone(&input.feed), Arc::clone(&input.capture));
        let mut r = Recorder::attached(input.capture, input.name, input.rate, session_rate, input.channels);
        r.channel_names = route.as_ref().map(crate::jack::Route::channel_names).unwrap_or_default();
        r._stream = Some(input.stream);
        Ok(r)
    }

    #[cfg(target_os = "freebsd")]
    pub fn open_with(_audio: &crate::AudioConfig, _player: &crate::Player) -> Result<Recorder, String> {
        Err("recording is not supported on this platform yet".into())
    }

    /// Create each take's file and start recording into them: a track's take records the given
    /// input channels into a new file.
    pub fn arm(&mut self, takes: Vec<(TrackId, Vec<usize>, PathBuf)>) -> Result<(), String> {
        if self.writer.is_some() {
            return Err("already recording".into());
        }
        let takes =
            takes.into_iter().map(|(id, inputs, path)| TakeWriter::create(id, inputs, path, self.sample_rate)).collect::<Result<Vec<_>, _>>()?;
        let live = takes.iter().map(|t| (t.id, Arc::clone(&t.live))).collect();
        self.capture.ring.clear();
        *self.capture.failed.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.capture.started.store(false, Ordering::Release);
        self.capture.armed.store(true, Ordering::Release);
        let (capture, channels) = (Arc::clone(&self.capture), self.channels);
        match std::thread::Builder::new().name("take writer".into()).spawn(move || write_takes(&capture, takes, channels)) {
            Ok(w) => self.writer = Some((w, live)),
            Err(e) => {
                self.capture.armed.store(false, Ordering::Release);
                return Err(e.to_string());
            }
        }
        Ok(())
    }

    /// The timeline position of the current take's first frame, once it has one.
    pub fn start_pos(&self) -> Option<Samples> {
        self.capture.started.load(Ordering::Acquire).then(|| self.capture.start_pos.load(Ordering::Acquire))
    }

    /// The input feed's latency (frames at the session rate) when the current take's first frame
    /// was captured, once it has one. Read there, live, because a drift feed's latency moves:
    /// the record offset for this take uses it rather than a figure from when the input opened.
    pub fn start_feed_latency(&self) -> Option<u32> {
        self.capture.started.load(Ordering::Acquire).then(|| self.capture.start_feed.load(Ordering::Relaxed))
    }

    /// The mix's own delay (frames) while the current take's first block played, once it has
    /// one. Read there, live: it drops while low-latency monitoring bypasses latent plugins.
    pub fn start_mix_latency(&self) -> Option<u32> {
        self.capture.started.load(Ordering::Acquire).then(|| self.capture.start_mix.load(Ordering::Relaxed))
    }

    /// What the take recording onto `track` holds so far.
    pub fn live_take(&self, track: TrackId) -> Option<MutexGuard<'_, LiveTake>> {
        let (_, take) = self.writer.as_ref()?.1.iter().find(|(id, _)| *id == track)?;
        Some(take.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Why the input can't go on, if it can't: the device went away or, while recording, stopped
    /// sending audio; its callback hit a bug; or writing a take failed (reported once).
    pub fn trouble(&mut self) -> Option<Trouble> {
        if let Some(e) = self.capture.failed.lock().unwrap_or_else(PoisonError::into_inner).take() {
            return Some(Trouble::Disk(e));
        }
        let name = &self.device_name;
        match self.capture.fault.load(Ordering::Relaxed) {
            GONE => return Some(Trouble::Device(format!("“{name}” was disconnected"))),
            BROKEN => return Some(Trouble::Device("the audio input hit an internal error (see the log)".into())),
            _ => {}
        }
        let beats = self.capture.beats.load(Ordering::Relaxed);
        if beats != self.seen.0 || !self.capture.armed.load(Ordering::Relaxed) {
            self.seen = (beats, Instant::now());
        }
        (self.seen.1.elapsed() >= STALL).then(|| Trouble::Device(format!("“{name}” stopped sending audio")))
    }

    /// Input the current take has lost to a slow disk so far, in milliseconds.
    pub fn lost_ms(&self) -> u64 {
        let frames = self.capture.dropped.load(Ordering::Relaxed) / self.channels.max(1);
        (frames as u64).saturating_mul(1000) / u64::from(self.sample_rate.max(1))
    }

    pub fn finish(&mut self) -> Takes {
        self.capture.armed.store(false, Ordering::Release);
        let (takes, problem) = self.writer.take().and_then(|(w, _)| w.join().ok()).unwrap_or_default();
        let lost = self.capture.dropped.swap(0, Ordering::Relaxed) / self.channels.max(1);
        let slow =
            (lost > 0).then(|| format!("{} ms of input lost: the disk was too slow", lost.saturating_mul(1000) / self.sample_rate.max(1) as usize));
        (takes, problem.or(slow))
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.finish();
    }
}

fn write_takes(shared: &Capture, mut takes: Vec<TakeWriter>, channels: usize) -> Takes {
    let (mut block, mut header_written, mut reported) = (Vec::new(), Instant::now(), false);
    loop {
        let stopping = !shared.armed.load(Ordering::Acquire);
        // The ring holds interleaved samples and a pop can end mid-frame: write whole frames and
        // keep the remainder for the next pop, so channels never shift.
        shared.ring.pop_all(&mut block);
        let whole = block.len() - block.len() % channels.max(1);
        takes.iter_mut().for_each(|t| t.write(&block[..whole], channels));
        block.drain(..whole);
        if !reported && let Some(e) = takes.iter().find_map(|t| t.error.clone()) {
            *shared.failed.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
            reported = true;
        }
        if stopping {
            break;
        }
        if header_written.elapsed() >= Duration::from_secs(1) {
            header_written = Instant::now();
            takes.iter_mut().for_each(TakeWriter::write_header);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (mut done, mut problem) = (Vec::new(), None);
    for mut t in takes {
        t.write_header();
        if t.frames == 0 {
            let _ = std::fs::remove_file(&t.path);
        } else {
            done.push((t.id, t.path));
        }
        problem = problem.or(t.error);
    }
    (done, problem)
}

/// One take's file. After an error it stops writing; its header keeps covering what was written.
struct TakeWriter {
    id: TrackId,
    inputs: Vec<usize>,
    path: PathBuf,
    file: File,
    rate: u32,
    frames: u64,
    error: Option<String>,
    live: Arc<Mutex<LiveTake>>,
}

impl TakeWriter {
    fn create(id: TrackId, inputs: Vec<usize>, path: PathBuf, rate: u32) -> Result<TakeWriter, String> {
        let file = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| File::create_new(&path));
        let file = file.map_err(|e| format!("{}: {e}", path.display()))?;
        let live = Arc::new(Mutex::new(vec![Vec::new(); inputs.len()]));
        let mut take = TakeWriter { id, inputs, path, file, rate, frames: 0, error: None, live };
        take.write_header();
        take.error.take().map_or(Ok(take), Err)
    }

    /// Append this take's channels of `block` (interleaved, `channels` wide).
    fn write(&mut self, block: &[f32], channels: usize) {
        let channels = channels.max(1);
        if self.error.is_some() {
            return;
        }
        let bytes: Vec<u8> = block
            .chunks_exact(channels)
            .flat_map(|f| self.inputs.iter().flat_map(move |&i| f.get(i).copied().unwrap_or(0.0).to_le_bytes()))
            .collect();
        match self.file.write_all(&bytes) {
            Ok(()) => {
                self.summarize(block, channels);
                self.frames += (block.len() / channels) as u64;
            }
            Err(e) => self.fail(e),
        }
    }

    /// Add this take's channels of `block` to its live take.
    fn summarize(&self, block: &[f32], channels: usize) {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        for (f, n) in block.chunks_exact(channels).zip(self.frames..) {
            let at = (n / LIVE_BLOCK as u64) as usize;
            for (blocks, &i) in live.iter_mut().zip(&self.inputs) {
                let v = f.get(i).copied().unwrap_or(0.0);
                match blocks.get_mut(at) {
                    Some(b) => *b = (b.0.min(v), b.1.max(v)),
                    None if at < LIVE_BLOCKS => blocks.push((v, v)),
                    None => {}
                }
            }
        }
    }

    fn write_header(&mut self) {
        let header = match soundcraft_audio_io::wav_float_header(self.inputs.len(), self.rate, self.frames) {
            Ok(h) => h,
            Err(e) => return self.fail(e),
        };
        let f = &mut self.file;
        if let Err(e) = f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&header)).and_then(|()| f.seek(SeekFrom::End(0))) {
            self.fail(e);
        }
    }

    fn fail(&mut self, e: impl std::fmt::Display) {
        self.error.get_or_insert_with(|| format!("{}: {e}", self.path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(path: &PathBuf) -> Vec<Vec<f32>> {
        soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), None).unwrap().1.channels
    }

    #[test]
    fn a_full_ring_owes_the_lost_samples_as_silence() {
        let shared = Capture::new(SampleRing::new(8));
        let (mut owed, mut out) = (0, Vec::new());
        shared.capture(&[1.0; 6], &mut owed);
        shared.capture(&[2.0; 4], &mut owed);
        shared.ring.pop_all(&mut out);
        shared.capture(&[3.0; 2], &mut owed);
        shared.ring.pop_all(&mut out);
        assert_eq!(out, [vec![1.0; 6], vec![2.0; 2], vec![0.0; 2], vec![3.0; 2]].concat());
        assert_eq!((owed, shared.dropped.load(Ordering::Relaxed)), (0, 2));
    }

    #[test]
    fn nan_and_infinite_input_records_as_silence() {
        let shared = Capture::new(SampleRing::new(8));
        let (mut owed, mut out) = (0, Vec::new());
        shared.capture(&[0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5], &mut owed);
        shared.ring.pop_all(&mut out);
        assert_eq!(out, [0.5, 0.0, 0.0, 0.0, -0.5]);
    }

    fn recorder(shared: &Arc<Capture>) -> Recorder {
        Recorder::attached(Arc::clone(shared), "Mic In".into(), 48_000, 48_000, 1)
    }

    #[test]
    fn a_bug_a_stall_or_a_lost_device_is_trouble() {
        let shared = Arc::new(Capture::new(SampleRing::new(64)));
        let mut r = recorder(&shared);
        shared.armed.store(true, Ordering::Release);
        assert_eq!(r.trouble(), None);
        r.seen.1 = Instant::now().checked_sub(STALL).unwrap();
        assert_eq!(r.trouble(), Some(Trouble::Device("“Mic In” stopped sending audio".into())));
        shared.guarded(|| {});
        assert_eq!(r.trouble(), None, "it sent again");
        shared.armed.store(false, Ordering::Release);
        r.seen.1 = Instant::now().checked_sub(STALL).unwrap();
        assert_eq!(r.trouble(), None, "a quiet input that isn't recording is fine");
        shared.guarded(|| std::panic::panic_any("a bug in the input"));
        let mut ran = false;
        shared.guarded(|| ran = true);
        assert!(!ran);
        assert_eq!(r.trouble(), Some(Trouble::Device("the audio input hit an internal error (see the log)".into())));
        let gone = Arc::new(Capture::new(SampleRing::new(64)));
        gone.fault.store(GONE, Ordering::Relaxed);
        assert_eq!(recorder(&gone).trouble(), Some(Trouble::Device("“Mic In” was disconnected".into())));
    }

    #[test]
    fn a_failed_write_is_trouble_while_still_recording() {
        let dir = std::env::temp_dir().join(format!("soundcraft-failed-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut take = TakeWriter::create(TrackId(1), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
        take.file = File::open(&take.path).unwrap();
        let shared = Arc::new(Capture::new(SampleRing::new(1 << 12)));
        shared.armed.store(true, Ordering::Release);
        let mut r = recorder(&shared);
        let sh = Arc::clone(&shared);
        let writer = std::thread::spawn(move || write_takes(&sh, vec![take], 1));
        shared.capture(&[0.5; 480], &mut 0);
        let trouble = (0..300).find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            r.trouble()
        });
        assert!(matches!(&trouble, Some(Trouble::Disk(e)) if e.contains("Vox_01.wav")), "{trouble:?}");
        assert_eq!(r.trouble(), None, "reported once");
        shared.armed.store(false, Ordering::Release);
        assert!(writer.join().unwrap().1.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn takes_stream_into_their_files() {
        let dir = std::env::temp_dir().join(format!("soundcraft-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let gtr = TakeWriter::create(TrackId(1), vec![1, 2], dir.join("Gtr_01.wav"), 48_000).unwrap();
        let mut vox = TakeWriter::create(TrackId(2), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
        assert!(TakeWriter::create(TrackId(3), vec![0], dir.join("Vox_01.wav"), 48_000).is_err(), "never overwrites a file");
        vox.write(&[0.5; 300], 3);
        vox.write_header();
        vox.write(&[0.25; 150], 3);
        assert_eq!(decode(&dir.join("Vox_01.wav"))[0].len(), 100, "the header covers what it has seen");

        let shared = Arc::new(Capture::new(SampleRing::new(1 << 16)));
        shared.armed.store(true, Ordering::Release);
        let sh = Arc::clone(&shared);
        let writer = std::thread::spawn(move || write_takes(&sh, vec![gtr, vox], 3));
        let input: Vec<f32> = (0..3 * 4_800).map(|i| i as f32 / 20_000.0).collect();
        let mut owed = 0;
        input.chunks(3 * 480).for_each(|block| shared.capture(block, &mut owed));
        shared.armed.store(false, Ordering::Release);
        let (done, problem) = writer.join().unwrap();
        let channel = |c: usize| input.iter().skip(c).step_by(3).copied().collect::<Vec<f32>>();
        assert_eq!((done.len(), problem), (2, None));
        assert_eq!(decode(&done[0].1), vec![channel(1), channel(2)]);
        assert_eq!(decode(&done[1].1), vec![[vec![0.5; 100], vec![0.25; 50], channel(0)].concat()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_take_sums_itself_up_for_drawing_while_it_records() {
        let dir = std::env::temp_dir().join(format!("soundcraft-live-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut take = TakeWriter::create(TrackId(4), vec![2, 0], dir.join("Gtr_01.wav"), 48_000).unwrap();
        let input: Vec<f32> = (0..300).flat_map(|f| [f as f32, -1.0, -(f as f32)]).collect();
        take.write(&input[..3 * 100], 3);
        take.write(&input[3 * 100..], 3);
        let live = take.live.lock().unwrap().clone();
        assert_eq!(live, [[(-127.0, 0.0), (-255.0, -128.0), (-299.0, -256.0)], [(0.0, 127.0), (128.0, 255.0), (256.0, 299.0)]]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An offline output callback: an armed mono track, a stereo input through `feed` into
    /// `capture`, recording, played from `from` on a 48 kHz device with blocks of up to 256.
    fn recording_state(feed: &Arc<InputFeed>, capture: &Arc<Capture>, from: Samples) -> crate::AudioState {
        counted_recording_state(feed, capture, from, None).0
    }

    /// [`recording_state`] started with `countoff`, and its shared transport and command channel.
    fn counted_recording_state(
        feed: &Arc<InputFeed>,
        capture: &Arc<Capture>,
        from: Samples,
        countoff: Option<crate::CountOff>,
    ) -> (crate::AudioState, Arc<crate::Shared>, std::sync::mpsc::Sender<crate::Cmd>) {
        use soundcraft_model::{ChannelFormat, Session, TrackKind};
        let mut s = Session::default();
        s.edit.click = false;
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().mixer.record_arm = true;
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(crate::Shared::new());
        let st = crate::AudioState::new(rx, Arc::clone(&shared), Arc::new(s), 48_000.0, 256, std::sync::mpsc::channel().0);
        tx.send(crate::Cmd::Input(Arc::clone(feed), vec![vec![0.0; 256]; feed.channels()])).unwrap();
        tx.send(crate::Cmd::Capture(Arc::clone(capture))).unwrap();
        tx.send(crate::Cmd::Recording(true)).unwrap();
        tx.send(crate::Cmd::Play { from, end: None, looped: None, countoff }).unwrap();
        (st, shared, tx)
    }

    /// One bar of four 300-frame beats: 1200 frames, which 128-frame callbacks cut mid-block.
    const SHORT_COUNTOFF: crate::CountOff = crate::CountOff { bars: 1, beats_per_bar: 4, beat: 300.0 };

    #[test]
    fn nothing_is_captured_during_the_count_off_and_the_take_starts_at_the_record_point() {
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        capture.armed.store(true, Ordering::Release);
        let (mut st, shared, _tx) = counted_recording_state(&feed, &capture, 1000, Some(SHORT_COUNTOFF));
        let probe = crate::TransportProbe { shared: Arc::clone(&shared) };
        let mut out = vec![0.0f32; 2 * 128];
        let mut sent = Vec::new();
        let mut clicks = 0.0f32;
        for b in 0..20 {
            let block = input_block(b);
            feed.push(&block);
            sent.extend_from_slice(&block);
            st.fill(&mut out, 2);
            if b < 9 {
                // Counting off (1152 of 1200 frames by the ninth callback): the transport holds at
                // the record point, its take clock still, and nothing is taken yet.
                clicks = clicks.max(out.iter().fold(0.0, |m, x| m.max(x.abs())));
                let r = probe.read();
                assert!(!r.playing && shared.counting.load(Ordering::Relaxed), "block {b}");
                assert_eq!((r.position, r.rolled), (1000, 0), "block {b}");
                assert!(!capture.started.load(Ordering::Acquire), "block {b}: no take during the count-off");
            }
        }
        assert!(clicks > 0.05, "the count-off is heard: {clicks}");
        assert!(!shared.counting.load(Ordering::Relaxed) && shared.playing.load(Ordering::Relaxed));
        // 20 callbacks of 128 frames: 1200 counting off, then 1360 rolled from the record point.
        let r = probe.read();
        assert_eq!((r.position, r.rolled), (1000 + 1360, 1360));
        assert_eq!(recorder(&capture).start_pos(), Some(1000), "the take starts at the record point");
        let mut got = Vec::new();
        capture.ring.pop_all(&mut got);
        assert_eq!(got, sent[2 * 1200..], "the input from the count-off's end on, none of it before");
    }

    #[test]
    fn stop_during_the_count_off_takes_nothing() {
        let dir = std::env::temp_dir().join(format!("soundcraft-countoff-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        let mut r = Recorder::attached(Arc::clone(&capture), "In".into(), 48_000, 48_000, 2);
        r.arm(vec![(TrackId(1), vec![0], dir.join("Vox_01.wav"))]).unwrap();
        let (mut st, shared, tx) = counted_recording_state(&feed, &capture, 1000, Some(SHORT_COUNTOFF));
        let mut out = vec![0.0f32; 2 * 128];
        for b in 0..5 {
            feed.push(&input_block(b));
            st.fill(&mut out, 2);
        }
        tx.send(crate::Cmd::Stop).unwrap();
        for b in 5..20 {
            feed.push(&input_block(b));
            st.fill(&mut out, 2);
        }
        assert!(!shared.counting.load(Ordering::Relaxed) && !shared.playing.load(Ordering::Relaxed));
        assert_eq!(shared.position.load(Ordering::Relaxed), 1000, "the transport never rolled");
        assert_eq!(r.start_pos(), None);
        let (done, problem) = r.finish();
        assert_eq!((done.len(), problem), (0, None), "no take");
        assert!(!dir.join("Vox_01.wav").exists(), "its empty file is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Block `b` of a stereo input: 128 interleaved frames, every sample distinct and non-zero.
    fn input_block(b: usize) -> Vec<f32> {
        (0..256).map(|i| 1e-4 * (b * 256 + i + 1) as f32).collect()
    }

    #[test]
    fn capture_starts_at_the_block_recording_starts() {
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        capture.armed.store(true, Ordering::Release);
        let mut st = recording_state(&feed, &capture, 1000);
        let mut out = vec![0.0f32; 2 * 128];
        let mut sent = Vec::new();
        for b in 0..10 {
            let block = input_block(b);
            feed.push(&block);
            sent.extend_from_slice(&block);
            st.fill(&mut out, 2);
        }
        let r = recorder(&capture);
        assert_eq!(r.start_pos(), Some(1000));
        let mut got = Vec::new();
        capture.ring.pop_all(&mut got);
        assert_eq!(got, sent, "the captured frames are the input, interleaved, in order");
    }

    #[test]
    fn a_take_keeps_the_feed_latency_at_its_start() {
        // Read live when the take's first block is captured, not when the input opened: a
        // cushioned feed's offset is only known once it has primed.
        let (feed, capture) = (InputFeed::fixed_cushioned(2, 256), Capture::for_rate(48_000, 2));
        assert_eq!(feed.extra_latency(), 0, "not primed yet");
        let r = recorder(&capture);
        assert_eq!(r.start_feed_latency(), None, "no take yet");
        capture.armed.store(true, Ordering::Release);
        let mut st = recording_state(&feed, &capture, 0);
        let mut out = vec![0.0f32; 2 * 128];
        for b in 0..10 {
            feed.push(&input_block(b));
            st.fill(&mut out, 2);
        }
        assert_eq!(r.start_feed_latency(), Some(128));
    }

    #[test]
    fn a_missing_input_block_records_silence_and_keeps_timing() {
        let (feed, capture) = (InputFeed::fixed(2, 256), Capture::for_rate(48_000, 2));
        capture.armed.store(true, Ordering::Release);
        let mut st = recording_state(&feed, &capture, 0);
        let mut out = vec![0.0f32; 2 * 128];
        let mut want = Vec::new();
        for b in 0..30 {
            if (10..20).contains(&b) {
                want.extend_from_slice(&[0.0; 256]);
            } else {
                let block = input_block(b);
                feed.push(&block);
                want.extend_from_slice(&block);
            }
            st.fill(&mut out, 2);
        }
        let mut got = Vec::new();
        capture.ring.pop_all(&mut got);
        assert_eq!(got.len(), 30 * 256, "the take is as long as what played");
        assert_eq!(got, want, "silence where the input was missing; the rest in place");
        assert_eq!(feed.extra_latency(), 0, "the offset did not move");
    }

    #[test]
    fn takes_are_written_at_the_session_rate() {
        let dir = std::env::temp_dir().join(format!("soundcraft-session-rate-take-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A 44.1 kHz input device (split, drift feed) into a 48 kHz session.
        let (feed, capture) = (InputFeed::drift(1, 44_100, 48_000, 256), Capture::for_rate(48_000, 1));
        let mut r = Recorder::attached(Arc::clone(&capture), "Mic In".into(), 44_100, 48_000, 1);
        assert_eq!((r.sample_rate, r.device_rate), (48_000, 44_100));
        r.arm(vec![(TrackId(1), vec![0], dir.join("Vox_01.wav"))]).unwrap();
        let mut st = recording_state(&feed, &capture, 0);
        let mut out = vec![0.0f32; 2 * 480];
        for _ in 0..20 {
            feed.push(&[0.25; 441]);
            st.fill(&mut out, 2);
        }
        let (done, problem) = r.finish();
        assert_eq!((done.len(), problem), (1, None));
        let (_, take) = soundcraft_audio_io::decode(&std::fs::read(&done[0].1).unwrap(), None).unwrap();
        assert_eq!((take.sample_rate, take.channels[0].len()), (48_000, 20 * 480), "48 kHz, every frame played");
        assert!(take.channels[0][19 * 480..].iter().all(|x| (x - 0.25).abs() < 1e-6), "the input, once the feed filled");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
