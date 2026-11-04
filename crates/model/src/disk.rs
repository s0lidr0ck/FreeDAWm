//! Audio played from disk: WAV files read block by block into one shared cache of decoded audio.
//!
//! A [`DiskFile`] is an audio file the session plays straight from disk (a native PCM or float
//! WAV at the session's rate). Its samples live in the process-wide block cache: pages of
//! [`BLOCK`] frames of one channel, each filled once by a reader and then shared. The cache uses
//! memory generously (see [`budget_bytes`]): a session that fits stays wholly cached and plays as
//! from memory; a longer one keeps what is near the playhead.
//!
//! Who reads:
//! - the **audio thread** through a [`Cursor`] in realtime mode: it only looks pages up (atomic
//!   loads; no lock, allocation, I/O or `Arc` drop). A page that isn't there yet reads as silence,
//!   counts one underrun ([`underruns`]) and is asked for (a lock-free request ring);
//! - the **disk thread** (one, started with the first file): every few milliseconds it loads what
//!   the audio thread asked for, then the next seconds after the playhead of the session in focus
//!   ([`focus`], [`set_playhead`]) and the loop start, then (while the cache has room) the rest of
//!   the session, nearest the playhead first;
//! - **offline work** (bounce, freeze, Process, analysis, the UI) through blocking reads
//!   ([`DiskFile::read`], a blocking [`Cursor`]): a missing block is read there and then.
//!
//! A page is reused safely without locks on the reading side: it is unpublished (its map entry
//! cleared, its key reset) and only refilled after a grace period far longer than any one audio
//! callback, and pages used in the last seconds are never evicted. A reader checks the page's key
//! after looking it up, so it never reads another block's samples.

use crate::Session;
use soundcraft_audio_io::StreamLayout;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

/// Frames per cache page (one channel): 32768, about 0.7 s at 48 kHz, 128 KiB of f32.
pub const BLOCK_SHIFT: u32 = 15;
pub const BLOCK: usize = 1 << BLOCK_SHIFT;
const PAGE_BYTES: usize = BLOCK * 4;
/// Key of a page holding nothing.
const EMPTY: u64 = u64::MAX;
/// Most channels a streamed file may have (the page key holds 8 bits of channel).
pub const MAX_CHANNELS: usize = 255;
/// How far ahead of the playhead the disk thread keeps audio ready while playing, seconds.
pub const READ_AHEAD_SECS: f64 = 4.0;
/// A page used this recently is never evicted.
const KEEP_RECENT: Duration = Duration::from_secs(3);
/// An unpublished page waits this long before it is refilled (far longer than any callback).
const GRACE: Duration = Duration::from_millis(1500);
/// How often the disk thread looks for work when nobody wakes it.
const POLL: Duration = Duration::from_millis(4);
/// Size of the ring of blocks the audio thread asks for.
const WANTS: usize = 256;

/// One cache page: [`BLOCK`] samples of one channel of one file block, as f32 bits.
struct Page {
    data: OnceLock<Box<[AtomicU32]>>,
    key: AtomicU64,
    /// [`Cache::tick`] when a reader last used (or a loader filled) it.
    used: AtomicU32,
}

struct Cache {
    pages: Box<[Page]>,
    /// Coarse clock (about 4 ms a tick) advanced by the disk thread; readers stamp pages with it.
    tick: AtomicU32,
    underruns: AtomicU64,
    /// Blocks the audio thread missed: page keys, `EMPTY` when free.
    wants: Box<[AtomicU64]>,
    want_at: AtomicUsize,
    playhead: AtomicI64,
    playing: AtomicBool,
    /// Bumped when the playhead jumps (a locate), so the planner looks again at once.
    moved: AtomicU64,
    inner: Mutex<Inner>,
    wake: Condvar,
    start: Instant,
    next_id: AtomicU32,
}

#[derive(Default)]
struct Inner {
    /// Pages with memory and no content.
    free: Vec<u32>,
    /// Pages unpublished at that time, reused after [`GRACE`].
    quarantine: VecDeque<(u32, Instant)>,
    /// Pages with memory allocated.
    allocated: usize,
    /// Every streamed file still alive (by id), so a dropped file's pages are freed.
    files: Vec<(u32, Weak<DiskFile>)>,
    /// The session whose audio is kept ready, and a counter bumped when it changes.
    focus: Option<Arc<Session>>,
    focus_gen: u64,
    thread: bool,
    /// Bytes allowed (pages × [`PAGE_BYTES`]).
    limit_pages: usize,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

static CACHE: OnceLock<Cache> = OnceLock::new();

/// The memory the block cache may use, bytes: `FREEDAWM_DISK_CACHE_MB` when set, else a quarter
/// of the computer's physical memory (at least 512 MiB, at most [`MAX_BUDGET`]), else (unknown)
/// 2 GiB: speed first, but a small PC is never pushed into paging.
pub fn budget_bytes() -> usize {
    static BUDGET: OnceLock<usize> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let env = ["FREEDAWM_DISK_CACHE_MB", "SOUNDCRAFT_DISK_CACHE_MB"]
            .iter()
            .find_map(|k| std::env::var(k).ok())
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|mb| *mb >= 16);
        if let Some(mb) = env {
            return mb.saturating_mul(1 << 20);
        }
        budget_for(soundcraft_sysmem::total_memory())
    })
}

/// Most the cache takes on its own (a session larger than this streams the rest).
pub const MAX_BUDGET: u64 = 6 << 30;

/// The cache budget for a computer with `total` bytes of memory (`None`: unknown).
pub fn budget_for(total: Option<u64>) -> usize {
    let cap = if usize::BITS >= 64 { MAX_BUDGET } else { 1 << 30 };
    let bytes = match total {
        Some(total) => (total / 4).clamp(512 << 20, cap),
        None => (2u64 << 30).min(cap),
    };
    usize::try_from(bytes).unwrap_or(usize::MAX)
}

/// Whether sessions play their WAV files from disk (the default); `FREEDAWM_STREAM=0` decodes
/// every file into memory as before (a fallback, and the baseline for measurements).
pub fn streaming_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        ["FREEDAWM_STREAM", "SOUNDCRAFT_STREAM"].iter().find_map(|k| std::env::var(k).ok()).is_none_or(|v| !matches!(v.trim(), "0" | "off" | "false"))
    })
}

fn cache() -> &'static Cache {
    CACHE.get_or_init(|| {
        let pages = (budget_bytes() / PAGE_BYTES).max(64);
        Cache {
            pages: (0..pages).map(|_| Page { data: OnceLock::new(), key: AtomicU64::new(EMPTY), used: AtomicU32::new(0) }).collect(),
            tick: AtomicU32::new(1),
            underruns: AtomicU64::new(0),
            wants: (0..WANTS).map(|_| AtomicU64::new(EMPTY)).collect(),
            want_at: AtomicUsize::new(0),
            playhead: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            moved: AtomicU64::new(0),
            inner: Mutex::new(Inner { limit_pages: pages, ..Inner::default() }),
            wake: Condvar::new(),
            start: Instant::now(),
            next_id: AtomicU32::new(1),
        }
    })
}

fn page_key(id: u32, ch: usize, block: usize) -> u64 {
    (u64::from(id & 0x00FF_FFFF) << 40) | ((ch as u64 & 0xFF) << 32) | (block as u64 & 0xFFFF_FFFF)
}

fn key_file(key: u64) -> u32 {
    (key >> 40) as u32
}

/// Blocks the audio thread found missing since start-up (each a short silence on one channel of
/// one clip): "disk too slow" when it grows while playing.
pub fn underruns() -> u64 {
    CACHE.get().map_or(0, |c| c.underruns.load(Ordering::Relaxed))
}

/// What the cache holds now: (bytes of decoded audio held, bytes allowed).
pub fn usage() -> (usize, usize) {
    match CACHE.get() {
        Some(c) => {
            let held = c.pages.iter().filter(|p| p.key.load(Ordering::Relaxed) != EMPTY).count();
            (held.saturating_mul(PAGE_BYTES), lock(&c.inner).limit_pages.saturating_mul(PAGE_BYTES))
        }
        None => (0, budget_bytes()),
    }
}

/// Lowers (or raises, up to [`budget_bytes`]) how much decoded audio the cache may hold. Pages
/// over a lowered limit are evicted as playback needs room.
pub fn set_limit_bytes(bytes: usize) {
    let c = cache();
    let mut inner = lock(&c.inner);
    inner.limit_pages = (bytes / PAGE_BYTES).clamp(16, c.pages.len());
}

/// Where the transport is (the audio thread may call it every block: two atomic stores, a third
/// when the position jumped).
pub fn set_playhead(pos: i64, playing: bool) {
    let Some(c) = CACHE.get() else { return };
    let was = c.playhead.swap(pos, Ordering::Relaxed);
    c.playing.store(playing, Ordering::Relaxed);
    // More than any one block moves it: a locate or a loop wrap.
    if pos.saturating_sub(was).abs() > 16_384 {
        c.moved.fetch_add(1, Ordering::Relaxed);
    }
}

/// The session whose audio the disk thread keeps ready (the one playing). Not for the audio
/// thread. Starts the disk thread when files are streamed.
pub fn focus(session: Option<Arc<Session>>) {
    let c = cache();
    {
        let mut inner = lock(&c.inner);
        inner.focus = session;
        inner.focus_gen = inner.focus_gen.wrapping_add(1);
    }
    c.wake.notify_all();
}

/// Loads (blocking) what playing from `pos` needs first: the next `secs` seconds of every clip
/// of `session`, the loop start too while looping, giving up after `timeout`. A transport
/// about to roll calls it so the first blocks are there. True when everything was loaded.
pub fn prepare(session: &Session, pos: i64, secs: f64, timeout: Duration) -> bool {
    let until = Instant::now() + timeout;
    let mut scratch = Scratch::default();
    let want = plan(session, pos, secs);
    for (file, block) in want {
        if Instant::now() > until {
            return false;
        }
        file.load(block, &mut scratch, Need::Urgent);
    }
    true
}

/// [`prepare`] for the session in [`focus`] (nothing to do without one).
pub fn prepare_focused(pos: i64, secs: f64, timeout: Duration) -> bool {
    let Some(c) = CACHE.get() else { return true };
    let focus = lock(&c.inner).focus.clone();
    focus.is_none_or(|s| prepare(&s, pos, secs, timeout))
}

/// A page a reader holds: its samples, and its key with the value it had when looked up.
#[derive(Clone, Copy)]
struct Held {
    data: &'static [AtomicU32],
    key: &'static AtomicU64,
    want: u64,
}

impl Held {
    /// Whether the page still holds the block it was looked up for: checked after reading its
    /// samples (a seqlock reader: the samples' loads come before the key's), so a page refilled
    /// meanwhile (a reader paused past the grace) is never taken for this block's audio.
    fn still_valid(&self) -> bool {
        fence(Ordering::Acquire);
        self.key.load(Ordering::Relaxed) == self.want
    }
}

/// The coarse clock now, advanced (it only moves forward) for every reader's next stamp too.
fn fresh_tick(c: &Cache) -> u32 {
    let t = ticks(c.start.elapsed()).max(1);
    c.tick.fetch_max(t, Ordering::Relaxed).max(t)
}

/// Positions read through the cache use these pieces of memory over and over.
#[derive(Default)]
struct Scratch {
    bytes: Vec<u8>,
    planar: Vec<Vec<f32>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    /// Playback needs it now: evict the least recently used page when full.
    Urgent,
    /// Filling the cache ahead: only into free room.
    Warm,
}

/// A WAV file played from disk (see the module docs).
pub struct DiskFile {
    id: u32,
    path: PathBuf,
    layout: StreamLayout,
    channels: usize,
    frames: usize,
    blocks: usize,
    /// `ch * blocks + block` → page + 1, 0 while not cached.
    map: Box<[AtomicU32]>,
    file: std::fs::File,
    /// Serialises reads on hosts without positional reads.
    #[cfg(not(any(unix, windows)))]
    seek: Mutex<()>,
}

impl std::fmt::Debug for DiskFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskFile").field("path", &self.path).field("channels", &self.channels).field("frames", &self.frames).finish()
    }
}

/// Why a file can't be played from disk (then it is decoded into memory, as before).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Not a WAV this reader streams (compressed, odd layout, too many channels).
    NotStreamable,
    /// The file couldn't be opened or read.
    Io(String),
}

impl DiskFile {
    /// Opens the WAV at `path` for streaming (its header alone is read).
    pub fn open(path: &Path) -> Result<Arc<DiskFile>, OpenError> {
        use std::io::Read;
        // A regular file only, checked before opening: opening a named pipe for reading waits
        // for a writer that never comes.
        if !std::fs::metadata(path).map_err(|e| OpenError::Io(format!("{}: {e}", path.display())))?.is_file() {
            return Err(OpenError::NotStreamable);
        }
        let mut file = std::fs::File::open(path).map_err(|e| OpenError::Io(format!("{}: {e}", path.display())))?;
        let len = file.metadata().map_err(|e| OpenError::Io(format!("{}: {e}", path.display())))?.len();
        if !file.metadata().is_ok_and(|m| m.is_file()) {
            return Err(OpenError::NotStreamable);
        }
        let mut prefix = Vec::with_capacity(64 * 1024);
        (&mut file).take(1 << 20).read_to_end(&mut prefix).map_err(|e| OpenError::Io(format!("{}: {e}", path.display())))?;
        let layout = soundcraft_audio_io::stream_layout(&prefix, len).ok_or(OpenError::NotStreamable)?;
        let channels = usize::from(layout.info.channels);
        if channels == 0 || channels > MAX_CHANNELS || layout.frame_bytes() == 0 {
            return Err(OpenError::NotStreamable);
        }
        let frames = usize::try_from(layout.info.frames).map_err(|_| OpenError::NotStreamable)?;
        let blocks = frames.div_ceil(BLOCK);
        if u32::try_from(blocks).is_err() {
            return Err(OpenError::NotStreamable);
        }
        let c = cache();
        let id = c.next_id.fetch_add(1, Ordering::Relaxed) & 0x00FF_FFFF;
        let f = Arc::new(DiskFile {
            id,
            path: path.to_path_buf(),
            layout,
            channels,
            frames,
            blocks,
            map: (0..channels.saturating_mul(blocks)).map(|_| AtomicU32::new(0)).collect(),
            file,
            #[cfg(not(any(unix, windows)))]
            seek: Mutex::new(()),
        });
        {
            let mut inner = lock(&c.inner);
            inner.files.push((id, Arc::downgrade(&f)));
            if !inner.thread {
                inner.thread = start_thread();
            }
        }
        Ok(f)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn frames(&self) -> usize {
        self.frames
    }
    pub fn sample_rate(&self) -> u32 {
        self.layout.info.sample_rate
    }
    /// Whether the samples are 32-bit float (what the project writes; a copy keeps every bit).
    pub fn is_float32(&self) -> bool {
        self.layout.info.sample_format == soundcraft_audio_io::SampleFormat::Float32
    }
    /// The file's BWF time stamp, if any.
    pub fn time_reference(&self) -> u64 {
        self.layout.info.bwf.as_ref().map_or(0, |b| b.time_reference)
    }

    fn slot(&self, ch: usize, block: usize) -> Option<&AtomicU32> {
        self.map.get(ch.checked_mul(self.blocks)?.checked_add(block)?)
    }

    /// The cached page of `(ch, block)`, checked to hold it. Lock- and allocation-free.
    fn page(&self, ch: usize, block: usize) -> Option<Held> {
        let c = CACHE.get()?;
        let p = self.slot(ch, block)?.load(Ordering::Acquire).checked_sub(1)?;
        let page = c.pages.get(p as usize)?;
        let want = page_key(self.id, ch, block);
        if page.key.load(Ordering::Acquire) != want {
            return None;
        }
        page.used.store(c.tick.load(Ordering::Relaxed), Ordering::Relaxed);
        page.data.get().map(|d| Held { data: d, key: &page.key, want })
    }

    fn cached(&self, block: usize) -> bool {
        (0..self.channels).all(|ch| self.slot(ch, block).is_some_and(|s| s.load(Ordering::Acquire) != 0))
    }

    /// Reads `buf.len()` bytes at `offset` (fewer at the end of the file).
    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
        let mut done = 0usize;
        while done < buf.len() {
            let Some(rest) = buf.get_mut(done..) else { break };
            let at = offset.saturating_add(done as u64);
            #[cfg(windows)]
            let n = std::os::windows::fs::FileExt::seek_read(&self.file, rest, at)?;
            #[cfg(unix)]
            let n = std::os::unix::fs::FileExt::read_at(&self.file, rest, at)?;
            #[cfg(not(any(unix, windows)))]
            let n = {
                use std::io::{Read, Seek};
                let _g = self.seek.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut f = &self.file;
                f.seek(std::io::SeekFrom::Start(at))?;
                f.read(rest)?
            };
            if n == 0 {
                break;
            }
            done += n;
        }
        Ok(done)
    }

    /// Decodes frames `[start, start + n)` of every channel into `scratch.planar` (each channel
    /// `n` long; what the file lacks reads as silence).
    fn decode(&self, start: usize, n: usize, scratch: &mut Scratch) -> bool {
        let fb = self.layout.frame_bytes();
        let avail = self.frames.saturating_sub(start).min(n);
        scratch.bytes.resize(avail.saturating_mul(fb), 0);
        let offset = self.layout.data_start.saturating_add((start as u64).saturating_mul(fb as u64));
        let got = match self.read_at(&mut scratch.bytes, offset) {
            Ok(got) => got,
            Err(_) => return false,
        };
        scratch.planar.resize_with(self.channels, Vec::new);
        for ch in scratch.planar.iter_mut() {
            ch.clear();
            ch.resize(n, 0.0);
        }
        let bytes = scratch.bytes.get(..got).unwrap_or_default();
        let mut outs: Vec<&mut [f32]> = scratch.planar.iter_mut().map(Vec::as_mut_slice).collect();
        self.layout.decode_into(bytes, &mut outs);
        true
    }

    /// Makes sure `block` (every channel) is cached. False when it could not be (a read error,
    /// or no page to spare for a [`Need::Warm`] load).
    fn load(&self, block: usize, scratch: &mut Scratch, need: Need) -> bool {
        if block >= self.blocks {
            return false;
        }
        if self.cached(block) {
            return true;
        }
        let c = cache();
        if need == Need::Warm && !has_room(c) {
            return false;
        }
        // The pages first: nothing is read when there is nowhere to put it.
        let missing: Vec<usize> = (0..self.channels).filter(|ch| self.slot(*ch, block).is_some_and(|s| s.load(Ordering::Acquire) == 0)).collect();
        let mut taken: Vec<(usize, u32)> = Vec::with_capacity(missing.len());
        {
            let mut inner = lock(&c.inner);
            for ch in missing {
                match take_page(c, &mut inner, need) {
                    Some(p) => taken.push((ch, p)),
                    None => {
                        inner.free.extend(taken.iter().map(|(_, p)| *p));
                        return false;
                    }
                }
            }
        }
        if !self.decode(block << BLOCK_SHIFT, BLOCK, scratch) {
            lock(&c.inner).free.extend(taken.iter().map(|(_, p)| *p));
            return false;
        }
        let now = fresh_tick(c);
        let mut spare: Vec<u32> = Vec::new();
        for (ch, p) in taken {
            let (Some(slot), Some(page)) = (self.slot(ch, block), c.pages.get(p as usize)) else {
                spare.push(p);
                continue;
            };
            // Its key was reset when it was unpublished; that comes before the new samples for
            // any reader (`Held::still_valid`).
            fence(Ordering::Release);
            let data = page.data.get_or_init(|| (0..BLOCK).map(|_| AtomicU32::new(0)).collect());
            if let Some(src) = scratch.planar.get(ch) {
                for (d, s) in data.iter().zip(src) {
                    d.store(s.to_bits(), Ordering::Relaxed);
                }
            }
            page.used.store(now, Ordering::Relaxed);
            page.key.store(page_key(self.id, ch, block), Ordering::Release);
            // Another loader may have published this block meanwhile: then this page goes back.
            if slot.compare_exchange(0, p + 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
                page.key.store(EMPTY, Ordering::Release);
                spare.push(p);
            }
        }
        if !spare.is_empty() {
            lock(&c.inner).free.extend(spare);
        }
        true
    }

    /// Reads channel `ch` from frame `start` into `out` (blocking: what isn't cached is read now
    /// and kept). Frames past the end read as silence; returns the frames inside the file.
    pub fn read(&self, ch: usize, start: usize, out: &mut [f32]) -> usize {
        let mut scratch = Scratch::default();
        self.read_with(ch, start, out, &mut scratch)
    }

    fn read_with(&self, ch: usize, start: usize, out: &mut [f32], scratch: &mut Scratch) -> usize {
        out.iter_mut().for_each(|x| *x = 0.0);
        if ch >= self.channels || start >= self.frames {
            return 0;
        }
        let n = out.len().min(self.frames - start);
        let mut i = 0usize;
        while i < n {
            let at = start + i;
            let (block, within) = (at >> BLOCK_SHIFT, at & (BLOCK - 1));
            let len = (BLOCK - within).min(n - i);
            let Some(dst) = out.get_mut(i..i + len) else { break };
            let mut page = self.page(ch, block);
            if page.is_none() && self.load(block, scratch, Need::Urgent) {
                page = self.page(ch, block);
            }
            let copied = page.and_then(|h| {
                let src = h.data.get(within..within + len)?;
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = f32::from_bits(s.load(Ordering::Relaxed));
                }
                h.still_valid().then_some(())
            });
            match copied {
                Some(()) => {}
                // No page to spare: straight from the file.
                None => {
                    if self.decode(at, len, scratch)
                        && let Some(src) = scratch.planar.get(ch)
                    {
                        for (d, v) in dst.iter_mut().zip(src) {
                            *d = *v;
                        }
                    }
                }
            }
            i += len;
        }
        n
    }

    /// The whole file decoded (blocking; for tools that need all of it at once).
    pub fn read_all(&self) -> Vec<Vec<f32>> {
        let mut scratch = Scratch::default();
        (0..self.channels)
            .map(|ch| {
                let mut v = vec![0.0; self.frames];
                self.read_with(ch, 0, &mut v, &mut scratch);
                v
            })
            .collect()
    }

    /// Reads the whole file once, block by block, without keeping it: `f` gets every channel of
    /// each block and how many frames of it are the file's. False on a read error.
    pub fn scan(&self, mut f: impl FnMut(&[Vec<f32>], usize)) -> bool {
        let mut scratch = Scratch::default();
        let mut at = 0usize;
        while at < self.frames {
            let n = (self.frames - at).min(BLOCK);
            if !self.decode(at, n, &mut scratch) {
                return false;
            }
            f(&scratch.planar, n);
            at += n;
        }
        true
    }

    /// Whether every block of the file is in the cache now.
    pub fn fully_cached(&self) -> bool {
        (0..self.blocks).all(|b| self.cached(b))
    }
}

/// Pages kept free (or on their way to free) once the cache is full, so playback's next blocks
/// always find one without waiting for an eviction's grace.
fn reserve(inner: &Inner) -> usize {
    (inner.limit_pages / 32).max(256).min(inner.limit_pages / 2)
}

fn limit(c: &Cache, inner: &Inner) -> usize {
    inner.limit_pages.min(c.pages.len())
}

/// Whether a warm (ahead-of-need) load may take a page now: never the reserve.
fn has_room(c: &Cache) -> bool {
    let inner = lock(&c.inner);
    inner.allocated.saturating_add(reserve(&inner)) < limit(c, &inner) || inner.free.len() > reserve(&inner)
}

/// A page to fill: a new one while under the limit, a free one, one past its grace (urgent loads
/// only: warm loads never take the reserve).
fn take_page(c: &Cache, inner: &mut Inner, need: Need) -> Option<u32> {
    // Warm loads leave the last pages (the reserve) to playback.
    let room = if need == Need::Warm { limit(c, inner).saturating_sub(reserve(inner)) } else { limit(c, inner) };
    if inner.allocated < room {
        let p = u32::try_from(inner.allocated).ok()?;
        inner.allocated += 1;
        return Some(p);
    }
    if need == Need::Warm {
        return if inner.free.len() > reserve(inner) { inner.free.pop() } else { None };
    }
    if let Some(p) = inner.free.pop() {
        return Some(p);
    }
    if inner.quarantine.front().is_some_and(|(_, at)| at.elapsed() >= GRACE) {
        return inner.quarantine.pop_front().map(|(p, _)| p);
    }
    evict(c, inner);
    None
}

/// Once the cache is full: unpublishes the least recently used pages (none used in the last
/// seconds) until the reserve is free or on its way. They are refilled after their grace.
fn evict(c: &Cache, inner: &mut Inner) {
    // Pages not allocated yet count as room too.
    let room = limit(c, inner).saturating_sub(inner.allocated) + inner.free.len() + inner.quarantine.len();
    let want = reserve(inner).saturating_sub(room);
    if want == 0 {
        return;
    }
    let now = fresh_tick(c);
    let recent = ticks(KEEP_RECENT);
    let mut old: Vec<(u32, u32)> = c
        .pages
        .iter()
        .take(inner.allocated)
        .enumerate()
        .filter(|(_, p)| p.key.load(Ordering::Relaxed) != EMPTY)
        .map(|(i, p)| (p.used.load(Ordering::Relaxed), i as u32))
        .filter(|(used, _)| now.wrapping_sub(*used) > recent)
        .collect();
    old.sort_unstable();
    let files: Vec<Arc<DiskFile>> = inner.files.iter().filter_map(|(_, w)| w.upgrade()).collect();
    for (_, p) in old.into_iter().take(want) {
        unpublish(c, inner, &files, p);
    }
    drop(files);
}

/// Takes page `p` out of the cache: its file's map entry cleared, its key reset, then quarantined.
fn unpublish(c: &Cache, inner: &mut Inner, files: &[Arc<DiskFile>], p: u32) {
    let Some(page) = c.pages.get(p as usize) else { return };
    let key = page.key.load(Ordering::Acquire);
    if key == EMPTY {
        return;
    }
    if let Some(f) = files.iter().find(|f| f.id == key_file(key)) {
        let (ch, block) = (((key >> 32) & 0xFF) as usize, (key & 0xFFFF_FFFF) as usize);
        if let Some(slot) = f.slot(ch, block) {
            let _ = slot.compare_exchange(p + 1, 0, Ordering::AcqRel, Ordering::Relaxed);
        }
    }
    page.key.store(EMPTY, Ordering::Release);
    inner.quarantine.push_back((p, Instant::now()));
}

fn ticks(d: Duration) -> u32 {
    u32::try_from(d.as_millis() / 4).unwrap_or(u32::MAX)
}

/// A reader of one channel of a source, position by position (the mix engine's clip renderer).
/// In realtime mode it never blocks: a block not cached reads as silence and is asked for.
pub struct Cursor<'a> {
    file: &'a DiskFile,
    ch: usize,
    blocking: bool,
    /// The page read now, cut to the file's frames, and its key to check after a run.
    page: Option<&'static [AtomicU32]>,
    held: Option<Held>,
    base: usize,
    /// The block a miss was last reported for (reported once per cursor and block).
    missed: usize,
    scratch: Option<Box<Scratch>>,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(file: &'a DiskFile, ch: usize, blocking: bool) -> Self {
        Cursor { file, ch, blocking, page: None, held: None, base: 0, missed: usize::MAX, scratch: None }
    }

    /// Sample `x`, `None` past the end of the file.
    #[inline]
    pub fn get(&mut self, x: usize) -> Option<f32> {
        if let Some(p) = self.page
            && let Some(s) = x.checked_sub(self.base).and_then(|i| p.get(i))
        {
            return Some(f32::from_bits(s.load(Ordering::Relaxed)));
        }
        self.seek(x)
    }

    /// Copies positions `[start, start + out.len())` into `out`: what [`Cursor::get`] reads
    /// there, silence past the end (and, in realtime mode, where a block isn't loaded yet).
    pub fn fill(&mut self, start: usize, out: &mut [f32]) {
        let mut i = 0usize;
        while i < out.len() {
            let x = start.saturating_add(i);
            let Some(rest) = out.get_mut(i..) else { return };
            if x >= self.file.frames {
                rest.fill(0.0);
                return;
            }
            let current = self.page.is_some_and(|p| x >= self.base && x - self.base < p.len());
            if !current {
                let v = self.seek(x);
                if self.page.is_none() {
                    // Not loaded: realtime reads the rest of the block as silence; a blocking
                    // read without a page to spare read this one sample from the file.
                    let n = if self.blocking { 1 } else { (BLOCK - (x & (BLOCK - 1))).min(rest.len()) };
                    if let Some(d) = rest.get_mut(..n) {
                        d.fill(0.0);
                    }
                    if let (true, Some(d)) = (self.blocking, rest.first_mut()) {
                        *d = v.unwrap_or(0.0);
                    }
                    i += n;
                    continue;
                }
            }
            let Some(p) = self.page else { return };
            let off = x - self.base;
            let n = p.len().saturating_sub(off).min(rest.len());
            if let (Some(d), Some(src)) = (rest.get_mut(..n), p.get(off..off + n)) {
                for (d, s) in d.iter_mut().zip(src.iter()) {
                    *d = f32::from_bits(s.load(Ordering::Relaxed));
                }
                // The page was refilled under this reader (paused past the grace): not this
                // block's audio, so silence, and the block is looked up again.
                if self.held.is_some_and(|h| !h.still_valid()) {
                    d.fill(0.0);
                    self.page = None;
                    self.held = None;
                }
            }
            i += n.max(1);
        }
    }

    #[cold]
    fn seek(&mut self, x: usize) -> Option<f32> {
        if x >= self.file.frames || self.ch >= self.file.channels {
            return None;
        }
        let block = x >> BLOCK_SHIFT;
        self.base = block << BLOCK_SHIFT;
        // The page cut to the file's frames, so a position past the end misses it.
        let valid = (self.file.frames - self.base).min(BLOCK);
        self.held = self.file.page(self.ch, block);
        self.page = self.held.and_then(|h| h.data.get(..valid));
        if self.page.is_none() {
            if self.blocking {
                let scratch = self.scratch.get_or_insert_with(Box::default);
                self.file.load(block, scratch, Need::Urgent);
                self.held = self.file.page(self.ch, block);
                self.page = self.held.and_then(|h| h.data.get(..valid));
                if self.page.is_none() {
                    // No page to spare: one sample straight from the file.
                    let mut one = [0.0f32];
                    self.file.read_with(self.ch, x, &mut one, scratch);
                    return Some(one[0]);
                }
            } else {
                if self.missed != block {
                    self.missed = block;
                    ask(self.file, block);
                }
                return Some(0.0);
            }
        }
        let i = x - self.base;
        self.page.and_then(|p| p.get(i)).map(|s| f32::from_bits(s.load(Ordering::Relaxed)))
    }
}

/// The audio thread found `block` of `file` missing: count it and ask the disk thread for it.
fn ask(file: &DiskFile, block: usize) {
    let Some(c) = CACHE.get() else { return };
    c.underruns.fetch_add(1, Ordering::Relaxed);
    let at = c.want_at.fetch_add(1, Ordering::Relaxed) % WANTS;
    if let Some(w) = c.wants.get(at) {
        w.store(page_key(file.id, 0, block), Ordering::Release);
    }
}

// ─── The disk thread ──────────────────────────────────────────────────────────────────────────

fn start_thread() -> bool {
    if cfg!(target_arch = "wasm32") {
        return false;
    }
    std::thread::Builder::new().name("freedawm-disk".into()).spawn(run).is_ok()
}

/// The blocks to load for playing `session` from `pos` for `secs` seconds (and from the loop start
/// while looping), nearest first.
fn plan(session: &Session, pos: i64, secs: f64) -> Vec<(Arc<DiskFile>, usize)> {
    let sr = session.sample_rate.as_f64();
    let len = (sr * secs).max(1.0) as i64;
    let mut windows = vec![(pos, pos.saturating_add(len))];
    if session.edit.loop_playback
        && let Some(l) = session.edit.loop_range.filter(|l| l.end > l.start)
    {
        windows.push((l.start, l.start.saturating_add(len).min(l.end)));
    }
    let mut out: Vec<(i64, Arc<DiskFile>, usize)> = Vec::new();
    for &(w0, w1) in &windows {
        collect(session, w0, w1, &mut out);
    }
    out.sort_by_key(|(d, f, b)| (*d, f.id, *b));
    let mut seen = std::collections::HashSet::new();
    out.retain(|(_, f, b)| seen.insert((f.id, *b)));
    out.into_iter().map(|(_, f, b)| (f, b)).collect()
}

/// Every streamed block the session plays in `[w0, w1)`, with its distance from `w0`.
fn collect(session: &Session, w0: i64, w1: i64, out: &mut Vec<(i64, Arc<DiskFile>, usize)>) {
    // Source positions `[src_from, src_to)` heard from timeline position `timeline` on.
    let mut add = |src: crate::SourceId, src_from: f64, src_to: f64, timeline: i64, stretch: f64| {
        let Some(f) = session.pool.get(src).and_then(|a| a.disk_file()) else { return };
        if src_to.is_nan() || src_to <= 0.0 || src_from >= f.frames as f64 || f.blocks == 0 {
            return;
        }
        let a = src_from.max(0.0) as usize >> BLOCK_SHIFT;
        let b = (src_to.min(f.frames as f64).max(0.0) as usize).saturating_sub(1) >> BLOCK_SHIFT;
        for block in a..=b.min(f.blocks - 1) {
            let ahead = (((block << BLOCK_SHIFT) as f64 - src_from).max(0.0) * stretch) as i64;
            let d = timeline.saturating_add(ahead).saturating_sub(w0).max(0);
            out.push((d, Arc::clone(f), block));
        }
    };
    for t in &session.tracks {
        if let Some(fz) = t.frozen_audio() {
            let from = w0.saturating_sub(fz.start) as f64;
            add(fz.source, from, from + w1.saturating_sub(w0) as f64, w0.max(fz.start), 1.0);
        }
        for clip in t.clips() {
            let crate::ClipContent::Audio { source, offset } = clip.content else { continue };
            let (c0, c1) = (clip.start.max(w0), clip.end().min(w1));
            if c1 <= c0 {
                continue;
            }
            let stretch = if clip.stretch.is_finite() && clip.stretch > 0.0 { clip.stretch } else { 1.0 };
            let from = offset as f64 + (c0 - clip.start) as f64 / stretch;
            let to = offset as f64 + (c1 - clip.start) as f64 / stretch + 2.0;
            add(source, from, to, c0, stretch);
            if let Some(k) = clip.clean_alt() {
                add(k.source, from - k.origin as f64, to - k.origin as f64, c0, stretch);
            }
        }
    }
}

/// Everything the session plays, nearest the playhead first (what fills a cache with room).
fn plan_all(session: &Session, pos: i64) -> Vec<(Arc<DiskFile>, usize)> {
    const FAR: i64 = 1 << 50;
    let mut after: Vec<(i64, Arc<DiskFile>, usize)> = Vec::new();
    collect(session, pos, FAR, &mut after);
    // Then what lies before the playhead, from the start.
    let mut before: Vec<(i64, Arc<DiskFile>, usize)> = Vec::new();
    collect(session, -FAR, pos, &mut before);
    after.extend(before.into_iter().map(|(d, f, b)| (d.saturating_add(FAR * 2), f, b)));
    after.sort_by_key(|(d, f, b)| (*d, f.id, *b));
    let mut seen = std::collections::HashSet::new();
    after.retain(|(_, f, b)| seen.insert((f.id, *b)));
    after.into_iter().map(|(_, f, b)| (f, b)).collect()
}

fn run() {
    let c = cache();
    let mut scratch = Scratch::default();
    let mut planned: Option<(u64, u64, i64, bool)> = None;
    let mut urgent: Vec<(Arc<DiskFile>, usize)> = Vec::new();
    let mut warm: Vec<(Arc<DiskFile>, usize)> = Vec::new();
    let mut warm_at = 0usize;
    let mut last_plan = Instant::now();
    loop {
        fresh_tick(c);
        // Free the pages of files nobody plays any more.
        let (focus, generation) = {
            let mut inner = lock(&c.inner);
            let dead: Vec<u32> = inner.files.iter().filter(|(_, w)| w.strong_count() == 0).map(|(id, _)| *id).collect();
            if !dead.is_empty() {
                inner.files.retain(|(_, w)| w.strong_count() > 0);
                for (i, page) in c.pages.iter().enumerate().take(inner.allocated) {
                    let key = page.key.load(Ordering::Relaxed);
                    if key != EMPTY && dead.contains(&key_file(key)) {
                        page.key.store(EMPTY, Ordering::Release);
                        inner.free.push(i as u32);
                    }
                }
            }
            evict(c, &mut inner);
            (inner.focus.clone(), inner.focus_gen)
        };
        // What the audio thread missed.
        let mut asked: Vec<u64> = Vec::new();
        for w in c.wants.iter() {
            let k = w.swap(EMPTY, Ordering::AcqRel);
            if k != EMPTY {
                asked.push(k);
            }
        }
        if !asked.is_empty() {
            let files: Vec<Arc<DiskFile>> = lock(&c.inner).files.iter().filter_map(|(_, w)| w.upgrade()).collect();
            for k in asked {
                if let Some(f) = files.iter().find(|f| f.id == key_file(k)) {
                    let block = (k & 0xFFFF_FFFF) as usize;
                    f.load(block, &mut scratch, Need::Urgent);
                    f.load(block + 1, &mut scratch, Need::Urgent);
                }
            }
        }
        let pos = c.playhead.load(Ordering::Relaxed);
        let playing = c.playing.load(Ordering::Relaxed);
        let moved = c.moved.load(Ordering::Relaxed);
        let stale = planned.is_none_or(|(g, m, p, was)| {
            g != generation || m != moved || was != playing || (pos - p).abs() > 12_000 || last_plan.elapsed() > Duration::from_millis(250)
        });
        if stale {
            last_plan = Instant::now();
            urgent = focus.as_deref().map(|s| plan(s, pos, READ_AHEAD_SECS)).unwrap_or_default();
            if planned.is_none_or(|(g, m, _, _)| g != generation || m != moved) {
                warm = focus.as_deref().map(|s| plan_all(s, pos)).unwrap_or_default();
                warm_at = 0;
            }
            planned = Some((generation, moved, pos, playing));
        }
        let mut busy = false;
        for (f, b) in urgent.iter() {
            if !f.cached(*b) {
                f.load(*b, &mut scratch, Need::Urgent);
                busy = true;
            }
            if c.wants.iter().any(|w| w.load(Ordering::Relaxed) != EMPTY) {
                break;
            }
        }
        // Then fill free room with the rest, a few blocks at a time.
        if !busy {
            let mut n = 0;
            while let Some((f, b)) = warm.get(warm_at) {
                if !f.cached(*b) {
                    if !f.load(*b, &mut scratch, Need::Warm) {
                        break;
                    }
                    n += 1;
                }
                warm_at += 1;
                if n >= 8 {
                    break;
                }
            }
            busy = n > 0;
        }
        drop(focus);
        if !busy {
            let inner = lock(&c.inner);
            let _ = c.wake.wait_timeout(inner, POLL);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn wav(tag: &str, frames: usize) -> PathBuf {
        let d = std::env::temp_dir().join(format!("freedawm-disk-unit-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let buf =
            soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![(0..frames).map(|i| (i % 100) as f32 / 100.0 + 0.01).collect()] };
        let opts = soundcraft_audio_io::EncodeOptions {
            format: soundcraft_audio_io::FileFormat::Wav,
            bit_depth: soundcraft_audio_io::BitDepth::Float32,
            dither: false,
            bwf: None,
        };
        let p = d.join("a.wav");
        std::fs::write(&p, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
        p
    }

    #[test]
    fn the_clock_moves_with_every_load_not_only_with_the_disk_thread() {
        let c = cache();
        let before = c.tick.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(40));
        let now = fresh_tick(c);
        assert!(now > before && c.tick.load(Ordering::Relaxed) >= now);
    }

    #[test]
    fn a_page_refilled_under_a_reader_reads_as_silence_not_another_blocks_audio() {
        let p = wav("refill", 1000);
        let f = DiskFile::open(&p).unwrap();
        let mut c = Cursor::new(&f, 0, true);
        assert!(c.get(10).is_some_and(|v| v > 0.0));
        let held = c.held.expect("a page is held");
        // The page changes hands (as after an eviction's grace) while the cursor still holds it.
        held.key.store(EMPTY, Ordering::Release);
        let mut out = vec![1.0f32; 100];
        c.fill(0, &mut out);
        assert!(out.iter().all(|v| *v == 0.0), "{:?}", &out[..4]);
        held.key.store(held.want, Ordering::Release);
        // Looked up again, it reads the file.
        let mut c = Cursor::new(&f, 0, true);
        c.fill(0, &mut out);
        assert!(out.iter().all(|v| *v > 0.0));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
