//! Waveform overviews read from the project's `Waveforms` cache on demand, so what they hold in
//! memory follows what is on screen, not how long the session is.
//!
//! A source played from disk in a saved project keeps only the coarse levels of its overview in
//! memory (from [`FINE_LEVELS`] up, about one byte for every 500 samples); its fine levels stay in
//! its cache file (`peak_cache`, which stores every level) and are read [`PAGE_BLOCKS`] blocks at
//! a time into one process-wide page cache with a byte budget ([`usage`], least recently used out
//! first). Drawing never waits: a page it needs and doesn't have is asked for and read on the
//! `peak-pages` thread, the coarser level stands in for it meanwhile, and the drawing asks to be
//! drawn again ([`soundcraft_audio_io::peaks::Peaks::spans_in_checked`]). Blocking readers (a
//! save writing the cache into another project) read the pages they need there and then.
//!
//! A cache file that went missing, was changed or doesn't read is made again from the audio once
//! (on that thread); if it still can't be read, the overview is held in memory from then on.
//! With no project folder (an unsaved session, a headless engine) overviews stay whole in memory.

use soundcraft_audio_io::peaks::{BLOCK_BYTES, FINE_LEVELS, FineLevels, LevelsBuilder, PAGE_BLOCKS, PeakPage, Peaks, level_sizes};
use soundcraft_model::disk::DiskFile;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

/// Default byte budget of the page cache (`FREEDAWM_PEAK_CACHE_MB` sets another): a screenful
/// of tracks at any zoom needs about a megabyte a channel.
pub const DEFAULT_BUDGET: usize = 96 << 20;
/// Most pages waiting to be read; older requests (scrolled past) are dropped first.
const MAX_QUEUE: usize = 2048;

/// Where each level of each channel starts in a cache file, and how long the file is.
#[derive(Debug, Clone)]
struct Layout {
    sizes: Vec<usize>,
    /// End of the level-0 part (what an old cache file holds).
    level0_end: u64,
    /// `[channel][level]` byte offset of the level's first block.
    at: Vec<Vec<u64>>,
    total: u64,
}

fn layout(channels: usize, frames: usize) -> Option<Layout> {
    let sizes = level_sizes(frames);
    let n0 = u64::try_from(*sizes.first()?).ok()?;
    let block = BLOCK_BYTES as u64;
    let section = n0.checked_mul(block)?.checked_add(16)?;
    let header = crate::peak_cache::HEADER as u64;
    let level0_end = section.checked_mul(channels as u64)?.checked_add(header)?;
    let mut next = level0_end.checked_add(crate::peak_cache::TRAILER.len() as u64)?;
    let mut at = Vec::with_capacity(channels);
    for c in 0..channels as u64 {
        let mut levels = vec![header.checked_add(section.checked_mul(c)?)?.checked_add(16)?];
        for n in sizes.iter().skip(1) {
            levels.push(next);
            next = next.checked_add(u64::try_from(*n).ok()?.checked_mul(block)?)?;
        }
        at.push(levels);
    }
    Some(Layout { sizes, level0_end, at, total: next })
}

/// One audio file's cache file, as its paged overviews read it.
#[derive(Debug)]
pub(crate) struct CachedFile {
    id: u64,
    project: PathBuf,
    file: PathBuf,
    rate: u32,
    cache: PathBuf,
    lay: Layout,
    audio: Arc<DiskFile>,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    /// The header the cache file must start with.
    header: Vec<u8>,
    /// Made again once already.
    rebuilt: bool,
    /// The overviews held whole, once the cache file can't be used.
    memory: Option<Vec<Peaks>>,
}

/// The fine levels of one channel of a [`CachedFile`].
#[derive(Debug)]
struct ChannelFine {
    file: Arc<CachedFile>,
    ch: usize,
}

impl FineLevels for ChannelFine {
    fn page(&self, level: usize, page: usize, wait: bool) -> Option<Arc<PeakPage>> {
        pages().get(&self.file, self.ch, level, page, wait)
    }
    fn unavailable(&self, level: usize, page: usize) -> bool {
        let key = Key { file: self.file.id, ch: self.ch, level, page };
        matches!(lock(&pages().inner).map.get(&key), Some(Slot::Failed))
    }
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The overviews of `audio` (the file `file`, opened at `rate`) paged from `project`'s cache, when
/// it has a cache file for the file as it is now that holds them; `None` otherwise (no cache, a
/// stale or damaged one, or audio too short to page: the caller builds or loads them whole). An
/// old cache file holding level 0 only is completed (the levels above are appended) on the way.
pub fn open(project: &Path, file: &Path, rate: u32, audio: &Arc<DiskFile>) -> Option<Vec<Peaks>> {
    let (channels, frames) = (audio.channels(), audio.frames());
    let lay = layout(channels, frames)?;
    if lay.sizes.len() <= FINE_LEVELS || channels == 0 {
        return None;
    }
    let cache = crate::peak_cache::path_for(project, file)?;
    crate::file_access::check_read(&cache).ok()?;
    let header = crate::peak_cache::expected(file, rate, channels)?;
    let mut f = std::fs::File::open(&cache).ok()?;
    let mut got = vec![0u8; header.len()];
    f.read_exact(&mut got).ok()?;
    if got != header {
        return None;
    }
    let n0 = lay.sizes.first().copied().unwrap_or(0) as u64;
    for c in 0..channels {
        let at = lay.at.get(c)?.first()?.checked_sub(16)?;
        let mut words = [0u8; 16];
        f.seek(SeekFrom::Start(at)).ok()?;
        f.read_exact(&mut words).ok()?;
        let word = |k: usize| words.get(k..k + 8).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
        if word(0)? != frames as u64 || word(8)? != n0 {
            return None;
        }
    }
    let len = f.metadata().ok()?.len();
    let coarse: Vec<Vec<PeakPage>> = if len == lay.level0_end {
        complete(&mut f, &cache, &lay, frames)?
    } else if len == lay.total {
        let mut magic = [0u8; 8];
        f.seek(SeekFrom::Start(lay.level0_end)).ok()?;
        f.read_exact(&mut magic).ok()?;
        if &magic != crate::peak_cache::TRAILER {
            return None;
        }
        (0..channels)
            .map(|c| {
                (FINE_LEVELS..lay.sizes.len())
                    .map(|l| {
                        let n = lay.sizes.get(l)?.checked_mul(BLOCK_BYTES)?;
                        let mut b = vec![0u8; n];
                        f.seek(SeekFrom::Start(*lay.at.get(c)?.get(l)?)).ok()?;
                        f.read_exact(&mut b).ok()?;
                        Some(PeakPage::from_le_bytes(&b))
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        return None;
    };
    drop(f);
    let cached = Arc::new(CachedFile {
        id: next_id(),
        project: project.to_path_buf(),
        file: file.to_path_buf(),
        rate,
        cache,
        lay,
        audio: Arc::clone(audio),
        state: Mutex::new(State { header, rebuilt: false, memory: None }),
    });
    coarse
        .into_iter()
        .enumerate()
        .map(|(ch, levels)| Peaks::paged(frames, levels, Arc::new(ChannelFine { file: Arc::clone(&cached), ch })))
        .collect::<Option<Vec<_>>>()
        .filter(|p| p.len() == channels)
}

/// An old cache file (level 0 only) completed: the levels above built from its level 0 and
/// appended. The coarse levels of each channel; `None` when it can't be read or written.
fn complete(f: &mut std::fs::File, cache: &Path, lay: &Layout, frames: usize) -> Option<Vec<Vec<PeakPage>>> {
    crate::file_access::check_write(cache).ok()?;
    let n0 = lay.sizes.first().copied()?;
    let mut uppers = Vec::with_capacity(lay.at.len());
    for levels in &lay.at {
        let mut b = LevelsBuilder::new(frames);
        f.seek(SeekFrom::Start(*levels.first()?)).ok()?;
        let mut left = n0;
        let mut buf = vec![0u8; PAGE_BLOCKS * 16 * BLOCK_BYTES];
        while left > 0 {
            let n = left.min(PAGE_BLOCKS * 16);
            let chunk = buf.get_mut(..n * BLOCK_BYTES)?;
            f.read_exact(chunk).ok()?;
            b.push_level0(chunk);
            left -= n;
        }
        uppers.push(b.finish()?);
    }
    let mut out = std::fs::OpenOptions::new().append(true).open(cache).ok()?;
    let written = (|| -> std::io::Result<()> {
        let mut w = std::io::BufWriter::with_capacity(1 << 20, &mut out);
        w.write_all(crate::peak_cache::TRAILER)?;
        for u in &uppers {
            for l in 1..u.num_levels() {
                u.write_level(l, &mut w)?;
            }
        }
        w.flush()
    })();
    if written.is_err() || out.metadata().ok()?.len() != lay.total {
        // Back to what it was, an old cache file (best effort; a damaged one is made again).
        let _ = out.set_len(lay.level0_end);
        return None;
    }
    let mut coarse = Vec::with_capacity(uppers.len());
    for u in uppers {
        let mut levels = Vec::new();
        for l in FINE_LEVELS..u.num_levels() {
            let mut b = Vec::new();
            u.write_level(l, &mut b).ok()?;
            levels.push(PeakPage::from_le_bytes(&b));
        }
        coarse.push(levels);
    }
    Some(coarse)
}

impl CachedFile {
    /// Page `page` of level `level` of channel `ch`, read now (blocking).
    fn read_page(&self, ch: usize, level: usize, page: usize) -> Option<PeakPage> {
        if ch >= self.lay.at.len() || page.checked_mul(PAGE_BLOCKS)? >= *self.lay.sizes.get(level)? {
            return None;
        }
        for _ in 0..2 {
            let header = {
                let st = lock(&self.state);
                if let Some(m) = &st.memory {
                    return m.get(ch)?.page_of(level, page, true).map(|p| (*p).clone());
                }
                st.header.clone()
            };
            if let Some(p) = self.read_file(&header, ch, level, page) {
                return Some(p);
            }
            self.recover(&header);
        }
        None
    }

    fn read_file(&self, header: &[u8], ch: usize, level: usize, page: usize) -> Option<PeakPage> {
        let at = *self.lay.at.get(ch)?.get(level)?;
        let n = *self.lay.sizes.get(level)?;
        let first = page.checked_mul(PAGE_BLOCKS)?;
        let count = n.checked_sub(first)?.min(PAGE_BLOCKS);
        if count == 0 {
            return None;
        }
        let mut f = std::fs::File::open(&self.cache).ok()?;
        let mut got = vec![0u8; header.len()];
        f.read_exact(&mut got).ok()?;
        if got != header || f.metadata().ok()?.len() != self.lay.total {
            return None;
        }
        f.seek(SeekFrom::Start(at.checked_add(u64::try_from(first.checked_mul(BLOCK_BYTES)?).ok()?)?)).ok()?;
        let mut b = vec![0u8; count.checked_mul(BLOCK_BYTES)?];
        f.read_exact(&mut b).ok()?;
        pages().reads.fetch_add(1, Ordering::Relaxed);
        Some(PeakPage::from_le_bytes(&b))
    }

    /// Whether the cache file now starts with `header` and has every level.
    fn usable(&self, header: &[u8]) -> bool {
        let Ok(mut f) = std::fs::File::open(&self.cache) else { return false };
        let mut got = vec![0u8; header.len()];
        f.read_exact(&mut got).is_ok() && got == header && f.metadata().is_ok_and(|m| m.len() == self.lay.total)
    }

    /// After a read of the cache file (expecting `seen`) failed: it was made again for the file as
    /// it is now (a save) and is used; else it is made again from the audio, once; else the
    /// overview is held in memory.
    fn recover(&self, seen: &[u8]) {
        let mut st = lock(&self.state);
        if st.header != seen || st.memory.is_some() {
            return;
        }
        let now = crate::peak_cache::expected(&self.file, self.rate, self.audio.channels());
        if let Some(now) = now.filter(|h| self.usable(h)) {
            st.header = now;
            return;
        }
        let made = soundcraft_model::SourceAudio::from_disk_scanned(Arc::clone(&self.audio)).peaks;
        if !st.rebuilt {
            st.rebuilt = true;
            log::info!("waveform cache {}: made again", self.cache.display());
            crate::peak_cache::store(&self.project, &self.file, self.rate, &made);
            if let Some(now) = crate::peak_cache::expected(&self.file, self.rate, self.audio.channels()).filter(|h| self.usable(h)) {
                st.header = now;
                return;
            }
        }
        log::info!("waveform cache {}: not usable, the overview is held in memory", self.cache.display());
        st.memory = Some(made);
    }
}

impl Drop for CachedFile {
    fn drop(&mut self) {
        if let Some(p) = PAGES.get() {
            let mut g = lock(&p.inner);
            let id = self.id;
            let mut freed = 0;
            g.map.retain(|k, s| {
                if k.file != id {
                    return true;
                }
                if let Slot::Ready { page, .. } = s {
                    freed += page.heap_bytes();
                }
                false
            });
            g.bytes = g.bytes.saturating_sub(freed);
            p.bytes.store(g.bytes, Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    file: u64,
    ch: usize,
    level: usize,
    page: usize,
}

enum Slot {
    Ready { page: Arc<PeakPage>, used: u64 },
    Loading,
    Failed,
}

#[derive(Default)]
struct Inner {
    map: HashMap<Key, Slot>,
    bytes: usize,
    tick: u64,
    queue: VecDeque<(Key, Arc<CachedFile>)>,
    started: bool,
}

struct Pages {
    inner: Mutex<Inner>,
    wake: Condvar,
    pending: AtomicUsize,
    bytes: AtomicUsize,
    budget: AtomicUsize,
    reads: AtomicU64,
}

static PAGES: OnceLock<Pages> = OnceLock::new();

fn pages() -> &'static Pages {
    PAGES.get_or_init(|| {
        let budget = std::env::var("FREEDAWM_PEAK_CACHE_MB")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .and_then(|mb| mb.checked_mul(1 << 20))
            .unwrap_or(DEFAULT_BUDGET);
        Pages {
            inner: Mutex::new(Inner::default()),
            wake: Condvar::new(),
            pending: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            budget: AtomicUsize::new(budget.max(1 << 20)),
            reads: AtomicU64::new(0),
        }
    })
}

impl Pages {
    fn get(&self, file: &Arc<CachedFile>, ch: usize, level: usize, page: usize, wait: bool) -> Option<Arc<PeakPage>> {
        let key = Key { file: file.id, ch, level, page };
        let mut dropped = Vec::new();
        {
            let mut g = lock(&self.inner);
            g.tick += 1;
            let tick = g.tick;
            match g.map.get_mut(&key) {
                Some(Slot::Ready { page, used }) => {
                    *used = tick;
                    return Some(Arc::clone(page));
                }
                Some(Slot::Loading) if !wait => return None,
                Some(Slot::Failed) if !wait => return None,
                None if !wait => {
                    g.map.insert(key, Slot::Loading);
                    g.queue.push_back((key, Arc::clone(file)));
                    self.pending.fetch_add(1, Ordering::Relaxed);
                    while g.queue.len() > MAX_QUEUE {
                        let Some((k, f)) = g.queue.pop_front() else { break };
                        if matches!(g.map.get(&k), Some(Slot::Loading)) {
                            g.map.remove(&k);
                        }
                        self.pending.fetch_sub(1, Ordering::Relaxed);
                        dropped.push(f);
                    }
                    if !g.started {
                        g.started = true;
                        if let Err(e) = std::thread::Builder::new().name("peak-pages".into()).spawn(worker) {
                            log::warn!("waveform page reader not started: {e}");
                        }
                    }
                    self.wake.notify_one();
                    drop(g);
                    drop(dropped);
                    return None;
                }
                _ => {}
            }
        }
        // Waiting: read it here, not through the cache (a whole file written out would push the
        // pages on screen out).
        file.read_page(ch, level, page).map(Arc::new)
    }

    /// Least recently used pages out until the cache is within its budget (`keep` stays).
    fn evict(&self, g: &mut Inner, keep: Key) {
        let budget = self.budget.load(Ordering::Relaxed);
        if g.bytes <= budget {
            return;
        }
        let mut ready: Vec<(u64, Key, usize)> = g
            .map
            .iter()
            .filter_map(|(k, s)| match s {
                Slot::Ready { page, used } if *k != keep => Some((*used, *k, page.heap_bytes())),
                _ => None,
            })
            .collect();
        ready.sort_unstable_by_key(|r| r.0);
        for (_, k, n) in ready {
            if g.bytes <= budget {
                break;
            }
            g.map.remove(&k);
            g.bytes = g.bytes.saturating_sub(n);
        }
    }
}

/// The `peak-pages` thread: reads the pages asked for, the latest first.
fn worker() {
    let p = pages();
    loop {
        let (key, file) = {
            let mut g = lock(&p.inner);
            loop {
                if let Some(x) = g.queue.pop_back() {
                    break x;
                }
                g = p.wake.wait(g).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let page = file.read_page(key.ch, key.level, key.page);
        let mut g = lock(&p.inner);
        if matches!(g.map.get(&key), Some(Slot::Loading)) {
            match page {
                Some(page) => {
                    g.bytes += page.heap_bytes();
                    g.tick += 1;
                    let used = g.tick;
                    g.map.insert(key, Slot::Ready { page: Arc::new(page), used });
                    p.evict(&mut g, key);
                }
                None => {
                    g.map.insert(key, Slot::Failed);
                }
            }
            p.bytes.store(g.bytes, Ordering::Relaxed);
            p.pending.fetch_sub(1, Ordering::Relaxed);
        }
        drop(g);
        drop(file);
    }
}

/// Pages asked for and not read yet (drawing asks to be drawn again while there are).
pub fn pending() -> usize {
    PAGES.get().map_or(0, |p| p.pending.load(Ordering::Relaxed))
}

/// Bytes the page cache holds and its budget.
pub fn usage() -> (usize, usize) {
    PAGES.get().map_or((0, DEFAULT_BUDGET), |p| (p.bytes.load(Ordering::Relaxed), p.budget.load(Ordering::Relaxed)))
}

/// Sets the page cache's budget (at least 1 MiB), evicting down to it.
pub fn set_budget_bytes(bytes: usize) {
    let p = pages();
    p.budget.store(bytes.max(1 << 20), Ordering::Relaxed);
    let mut g = lock(&p.inner);
    let keep = Key { file: 0, ch: 0, level: 0, page: 0 };
    p.evict(&mut g, keep);
    p.bytes.store(g.bytes, Ordering::Relaxed);
}

/// Pages read from cache files so far (on demand: what was on screen).
pub fn reads() -> u64 {
    PAGES.get().map_or(0, |p| p.reads.load(Ordering::Relaxed))
}

/// Waits until no page is waiting to be read, at most `timeout`; whether none is.
pub fn wait_idle(timeout: std::time::Duration) -> bool {
    let end = std::time::Instant::now() + timeout;
    while pending() > 0 {
        if std::time::Instant::now() >= end {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    true
}

/// `audio` with its overviews paged from `project`'s cache when it plays from disk, holds them
/// whole and the cache has them for its file `file` as it is now; else `audio` as it is.
pub fn paged_audio(project: &Path, file: &Path, rate: u32, audio: soundcraft_model::SourceAudio) -> soundcraft_model::SourceAudio {
    let Some(f) = audio.disk_file().filter(|_| !audio.peaks.iter().all(Peaks::is_paged)).cloned() else { return audio };
    match open(project, file, rate, &f) {
        Some(peaks) => soundcraft_model::SourceAudio::from_disk(f, peaks).unwrap_or(audio),
        None => audio,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_places_every_level_after_level_zero() {
        let lay = layout(2, 100_000).unwrap();
        let n = lay.sizes.clone();
        assert_eq!(n[0], 3125);
        let h = crate::peak_cache::HEADER as u64;
        assert_eq!(lay.at[0][0], h + 16);
        assert_eq!(lay.at[1][0], h + 16 + 3125 * 12 + 16);
        assert_eq!(lay.level0_end, h + 2 * (16 + 3125 * 12));
        assert_eq!(lay.at[0][1], lay.level0_end + 8);
        let upper: u64 = n.iter().skip(1).map(|x| *x as u64 * 12).sum();
        assert_eq!(lay.at[1][1], lay.level0_end + 8 + upper);
        assert_eq!(lay.total, lay.level0_end + 8 + 2 * upper);
        assert!(layout(1, 0).is_none());
    }
}
