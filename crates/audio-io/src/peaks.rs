//! Multi-resolution overview for waveform drawing.
//!
//! Level 0 sums up each block of [`BLOCK`] samples (min, max and sum of squares); each following
//! level merges [`FANOUT`] blocks of the level below, until a level has a single block. Range
//! queries combine the coarsest whole blocks that fit inside the range and read the partial blocks
//! at its edges from the samples, so a query is exact and costs O(log n) regardless of zoom.
//!
//! An overview is held whole in memory, or **paged**: only its coarse levels (from
//! [`FINE_LEVELS`] up, about 1/256 of the whole) are in memory and its fine levels are read
//! [`PAGE_BLOCKS`] blocks at a time through a [`FineLevels`] (the engine's waveform cache file and
//! a bounded page cache). A drawing query never waits for a page: where one is not loaded yet the
//! finest coarser block at hand stands in for it (a slightly wider column for a moment), the page
//! is asked for, and the query says it was not complete so the caller draws again. Blocking
//! queries ([`Peaks::minmax`], [`Peaks::write_level`]) wait for the pages they need.
//!
//! The columns of a waveform drawn zoomed out ([`Peaks::spans_in`] with a `step` of at least
//! [`COLUMN_SHARE`] times a level's block) are summed from whole blocks of the finest such level,
//! the column's ends rounded outward to them: at most 1/[`COLUMN_SHARE`] of a column wider on each
//! side (a fraction of a pixel), and the fine pages read are those of the visible range only.
//! Held whole or paged, an overview answers every query the same.

use std::sync::Arc;

/// Samples per level-0 block.
pub const BLOCK: usize = 32;
/// Blocks of level `n` merged into one block of level `n + 1`.
pub const FANOUT: usize = 4;
/// Upper bound on the number of spans [`Peaks::spans`] will produce.
pub const MAX_COLUMNS: usize = 1 << 20;
/// Levels `0..FINE_LEVELS` of a paged overview are read a page at a time; the levels above stay
/// in memory (a block of level 4 sums up 8192 samples).
pub const FINE_LEVELS: usize = 4;
/// Blocks per page of a paged level.
pub const PAGE_BLOCKS: usize = 4096;
/// Bytes one block takes in a cache file (min, max and sum of squares, little-endian `f32`).
pub const BLOCK_BYTES: usize = 12;
/// A zoomed-out column `step` samples wide is summed from blocks of at most `step / COLUMN_SHARE`
/// samples.
pub const COLUMN_SHARE: f64 = 8.0;

/// What a run of samples sums up to; NaN samples count for nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Block {
    min: f32,
    max: f32,
    squares: f32,
}

impl Block {
    const EMPTY: Block = Block { min: f32::INFINITY, max: f32::NEG_INFINITY, squares: 0.0 };

    fn point(s: f32) -> Block {
        Block { min: s, max: s, squares: if s.is_finite() { s * s } else { 0.0 } }
    }

    fn of(samples: &[f32]) -> Block {
        samples.iter().fold(Block::EMPTY, |b, &s| b.merge(Block::point(s)))
    }

    fn merge(self, o: Block) -> Block {
        Block { min: self.min.min(o.min), max: self.max.max(o.max), squares: self.squares + o.squares }
    }

    /// (min, max), or (0, 0) when nothing was summed up.
    fn range(self) -> (f32, f32) {
        if self.min > self.max { (0.0, 0.0) } else { (self.min, self.max) }
    }

    fn write_le(self, out: &mut Vec<u8>) {
        for v in [self.min, self.max, self.squares] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn from_le(c: &[u8]) -> Block {
        let f = |a: usize| c.get(a..a + 4).and_then(|b| b.try_into().ok()).map_or(0.0, f32::from_le_bytes);
        Block { min: f(0), max: f(4), squares: f(8) }
    }
}

/// Up to [`PAGE_BLOCKS`] consecutive blocks of one level of an overview (a page of a paged one).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PeakPage(Vec<Block>);

impl PeakPage {
    /// The blocks in `bytes` as a cache file stores them ([`BLOCK_BYTES`] each; a partial block at
    /// the end is left out).
    pub fn from_le_bytes(bytes: &[u8]) -> PeakPage {
        PeakPage(bytes.chunks_exact(BLOCK_BYTES).map(Block::from_le).collect())
    }
    /// Number of blocks.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Memory the page holds.
    pub fn heap_bytes(&self) -> usize {
        self.0.capacity() * std::mem::size_of::<Block>()
    }
}

/// Where a paged overview's fine levels (`0..FINE_LEVELS`) are read from: page `page` of level
/// `level` holds that level's blocks `[page * PAGE_BLOCKS, (page + 1) * PAGE_BLOCKS)` (fewer in the
/// last page). With `wait`, the page is read now (blocking) when it is not at hand; without, it
/// is returned only when already loaded, and otherwise asked for (`None` now).
pub trait FineLevels: Send + Sync + std::fmt::Debug {
    fn page(&self, level: usize, page: usize, wait: bool) -> Option<Arc<PeakPage>>;
    /// Whether the page, not returned, never will be (it could not be read): the coarser level
    /// stands in for good and a drawing does not ask to be drawn again for it.
    fn unavailable(&self, _level: usize, _page: usize) -> bool {
        false
    }
}

/// The number of blocks of each level of the overview of `len` samples (none when empty).
pub fn level_sizes(len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let mut sizes = vec![len.div_ceil(BLOCK)];
    while let Some(&last) = sizes.last().filter(|n| **n > 1) {
        sizes.push(last.div_ceil(FANOUT));
    }
    sizes
}

/// Multi-resolution overview of one channel for waveform drawing (see the module docs).
#[derive(Clone)]
pub struct Peaks {
    len: usize,
    store: Store,
}

#[derive(Clone)]
enum Store {
    Full(Vec<Vec<Block>>),
    Paged(Arc<Paged>),
}

/// A paged overview: every level's size, the levels from [`FINE_LEVELS`] up, and where the fine
/// ones are read from.
struct Paged {
    sizes: Vec<usize>,
    coarse: Vec<Vec<Block>>,
    fine: Arc<dyn FineLevels>,
}

impl Default for Peaks {
    fn default() -> Self {
        Peaks { len: 0, store: Store::Full(Vec::new()) }
    }
}

impl std::fmt::Debug for Peaks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peaks").field("len", &self.len).field("levels", &self.num_levels()).field("paged", &self.is_paged()).finish()
    }
}

/// Equal when they sum up the same blocks (a paged overview's fine levels are read for it, waiting).
impl PartialEq for Peaks {
    fn eq(&self, o: &Peaks) -> bool {
        if self.len != o.len || self.num_levels() != o.num_levels() {
            return false;
        }
        if let (Store::Full(a), Store::Full(b)) = (&self.store, &o.store) {
            return a == b;
        }
        (0..self.num_levels()).all(|l| {
            (0..self.level_len(l).div_ceil(PAGE_BLOCKS)).all(|p| match (self.page_of(l, p, true), o.page_of(l, p, true)) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            })
        })
    }
}

/// Samples a query reads: `data` holds positions `[off, off + data.len())` of a channel `len`
/// long; positions outside it read as nothing (the overview's blocks stand for them).
#[derive(Clone, Copy)]
struct View<'a> {
    data: &'a [f32],
    off: usize,
    len: usize,
}

impl<'a> View<'a> {
    fn whole(samples: &'a [f32]) -> Self {
        View { data: samples, off: 0, len: samples.len() }
    }
    fn get(&self, i: usize) -> Option<f32> {
        self.data.get(i.checked_sub(self.off)?).copied()
    }
    fn slice(&self, a: usize, b: usize) -> &'a [f32] {
        let (a, b) = (a.saturating_sub(self.off).min(self.data.len()), b.saturating_sub(self.off).min(self.data.len()));
        self.data.get(a..b.max(a)).unwrap_or_default()
    }
}

/// The pages one query has looked up (two per fine level: a column's two ends), and whether a
/// block it needed was not loaded.
struct Lookup {
    pages: [[Option<(usize, Option<Arc<PeakPage>>)>; 2]; FINE_LEVELS],
    /// The last page of each level found missing, and whether it never will be loaded.
    gone: [Option<(usize, bool)>; FINE_LEVELS],
    wait: bool,
    missing: bool,
}

impl Lookup {
    fn new(wait: bool) -> Self {
        Lookup { pages: Default::default(), gone: Default::default(), wait, missing: false }
    }
}

/// Builds a [`Peaks`] from a channel's samples given piece by piece (a file read in blocks): the
/// same overview [`Peaks::build`] makes from all of them at once.
#[derive(Debug, Clone, Default)]
pub struct PeaksBuilder {
    level0: Vec<Block>,
    part: Vec<f32>,
    len: usize,
}

impl PeaksBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    /// The next samples of the channel.
    pub fn push(&mut self, mut samples: &[f32]) {
        self.len += samples.len();
        if !self.part.is_empty() {
            let take = (BLOCK - self.part.len()).min(samples.len());
            self.part.extend_from_slice(samples.get(..take).unwrap_or_default());
            samples = samples.get(take..).unwrap_or_default();
            if self.part.len() == BLOCK {
                self.level0.push(Block::of(&self.part));
                self.part.clear();
            }
        }
        let whole = samples.len() / BLOCK * BLOCK;
        self.level0.extend(samples.get(..whole).unwrap_or_default().chunks(BLOCK).map(Block::of));
        self.part.extend_from_slice(samples.get(whole..).unwrap_or_default());
    }
    pub fn finish(mut self) -> Peaks {
        if !self.part.is_empty() {
            self.level0.push(Block::of(&self.part));
        }
        Peaks::from_level0(self.level0, self.len)
    }
}

/// Builds the levels above level 0 of an overview of `len` samples from its level-0 blocks given
/// in order, as a cache file stores them: the same blocks [`Peaks::build`] makes, without holding
/// level 0 (an old cache file that has only level 0 is completed with it).
#[derive(Debug, Clone)]
pub struct LevelsBuilder {
    len: usize,
    sizes: Vec<usize>,
    /// Levels 1 and up (index 0 stays empty).
    levels: Vec<Vec<Block>>,
    /// Each level's partly merged block of the level above and how many it has merged.
    acc: Vec<(Block, usize)>,
    pushed: usize,
}

impl LevelsBuilder {
    pub fn new(len: usize) -> Self {
        let sizes = level_sizes(len);
        let levels = sizes.iter().enumerate().map(|(l, n)| if l == 0 { Vec::new() } else { Vec::with_capacity(*n) }).collect();
        LevelsBuilder { len, acc: vec![(Block::EMPTY, 0); sizes.len()], sizes, levels, pushed: 0 }
    }
    /// The next level-0 blocks ([`BLOCK_BYTES`] each).
    pub fn push_level0(&mut self, bytes: &[u8]) {
        for c in bytes.chunks_exact(BLOCK_BYTES) {
            self.pushed += 1;
            self.push(0, Block::from_le(c));
        }
    }
    fn push(&mut self, mut lvl: usize, mut b: Block) {
        let n = self.sizes.len();
        while lvl < n {
            if let Some(l) = self.levels.get_mut(lvl).filter(|_| lvl > 0) {
                l.push(b);
            }
            let Some(slot) = self.acc.get_mut(lvl).filter(|_| lvl + 1 < n) else { return };
            slot.0 = slot.0.merge(b);
            slot.1 += 1;
            if slot.1 < FANOUT {
                return;
            }
            b = slot.0;
            *slot = (Block::EMPTY, 0);
            lvl += 1;
        }
    }
    /// The levels above level 0, or `None` when the blocks given were not level 0 of `len` samples.
    pub fn finish(mut self) -> Option<UpperLevels> {
        if self.pushed != self.sizes.first().copied().unwrap_or(0) {
            return None;
        }
        for lvl in 0..self.sizes.len().saturating_sub(1) {
            if let Some(&(b, k)) = self.acc.get(lvl).filter(|a| a.1 > 0) {
                debug_assert!(k < FANOUT);
                self.acc[lvl] = (Block::EMPTY, 0);
                self.push(lvl + 1, b);
            }
        }
        let ok = self.levels.iter().zip(&self.sizes).skip(1).all(|(l, n)| l.len() == *n);
        ok.then_some(UpperLevels { len: self.len, sizes: self.sizes, levels: self.levels })
    }
}

/// The levels above level 0 of an overview ([`LevelsBuilder`]).
#[derive(Debug, Clone)]
pub struct UpperLevels {
    len: usize,
    sizes: Vec<usize>,
    levels: Vec<Vec<Block>>,
}

impl UpperLevels {
    /// Writes level `level` (1 and up) as a cache file stores it.
    pub fn write_level(&self, level: usize, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        let mut buf = Vec::with_capacity(PAGE_BLOCKS * BLOCK_BYTES);
        for chunk in self.levels.get(level).map_or(&[][..], Vec::as_slice).chunks(PAGE_BLOCKS) {
            buf.clear();
            chunk.iter().for_each(|b| b.write_le(&mut buf));
            w.write_all(&buf)?;
        }
        Ok(())
    }
    /// The paged overview these levels are the coarse part of, its fine levels read from `fine`.
    pub fn into_paged(self, fine: Arc<dyn FineLevels>) -> Option<Peaks> {
        let coarse: Vec<PeakPage> = self.levels.into_iter().skip(FINE_LEVELS).map(PeakPage).collect();
        Peaks::paged(self.len, coarse, fine)
    }
    pub fn num_levels(&self) -> usize {
        self.sizes.len()
    }
}

impl Peaks {
    /// Build the overview of `samples` (one channel). NaN samples are ignored.
    pub fn build(samples: &[f32]) -> Peaks {
        Peaks::from_level0(samples.chunks(BLOCK).map(Block::of).collect(), samples.len())
    }

    fn from_level0(level0: Vec<Block>, len: usize) -> Peaks {
        if len == 0 {
            return Peaks::default();
        }
        let mut levels: Vec<Vec<Block>> = vec![level0];
        while let Some(last) = levels.last().filter(|l| l.len() > 1) {
            let next = last.chunks(FANOUT).map(|c| c.iter().fold(Block::EMPTY, |a, &b| a.merge(b))).collect();
            levels.push(next);
        }
        Peaks { len, store: Store::Full(levels) }
    }

    /// A paged overview of `len` samples: `coarse` holds its levels from [`FINE_LEVELS`] up, each
    /// whole; the levels below are read from `fine`. `None` when the levels don't fit `len` (or
    /// the overview has no level above the fine ones: hold it whole).
    pub fn paged(len: usize, coarse: Vec<PeakPage>, fine: Arc<dyn FineLevels>) -> Option<Peaks> {
        let sizes = level_sizes(len);
        let upper = sizes.get(FINE_LEVELS..).unwrap_or_default();
        if upper.is_empty() || coarse.len() != upper.len() || coarse.iter().zip(upper).any(|(c, n)| c.len() != *n) {
            return None;
        }
        Some(Peaks { len, store: Store::Paged(Arc::new(Paged { sizes, coarse: coarse.into_iter().map(|p| p.0).collect(), fine })) })
    }

    /// Whether its fine levels are read a page at a time ([`Peaks::paged`]).
    pub fn is_paged(&self) -> bool {
        matches!(self.store, Store::Paged(_))
    }

    /// Memory the overview itself holds (a paged one's pages are the page cache's).
    pub fn memory_bytes(&self) -> usize {
        let levels = match &self.store {
            Store::Full(l) => l,
            Store::Paged(p) => &p.coarse,
        };
        levels.iter().map(|l| l.capacity() * std::mem::size_of::<Block>()).sum()
    }

    /// Number of blocks of level `level`.
    pub fn level_len(&self, level: usize) -> usize {
        match &self.store {
            Store::Full(l) => l.get(level).map_or(0, Vec::len),
            Store::Paged(p) => p.sizes.get(level).copied().unwrap_or(0),
        }
    }

    /// Page `page` of level `level` ([`PAGE_BLOCKS`] blocks; see [`FineLevels`] for `wait`).
    pub fn page_of(&self, level: usize, page: usize, wait: bool) -> Option<Arc<PeakPage>> {
        let slice = |l: &Vec<Block>| -> Option<Arc<PeakPage>> {
            let a = page.checked_mul(PAGE_BLOCKS)?;
            let b = a.saturating_add(PAGE_BLOCKS).min(l.len());
            (a < b).then(|| Arc::new(PeakPage(l.get(a..b).unwrap_or_default().to_vec())))
        };
        match &self.store {
            Store::Full(levels) => slice(levels.get(level)?),
            Store::Paged(p) if level < FINE_LEVELS => {
                (page.checked_mul(PAGE_BLOCKS)? < p.sizes.get(level).copied().unwrap_or(0)).then(|| p.fine.page(level, page, wait)).flatten()
            }
            Store::Paged(p) => slice(p.coarse.get(level - FINE_LEVELS)?),
        }
    }

    /// Writes level `level`'s blocks as a cache file stores them ([`BLOCK_BYTES`] each), a page at a
    /// time (a paged overview's fine pages are read for it, waiting); an error when one can't be.
    pub fn write_level(&self, level: usize, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        let mut buf = Vec::with_capacity(PAGE_BLOCKS * BLOCK_BYTES);
        for page in 0..self.level_len(level).div_ceil(PAGE_BLOCKS) {
            let p = self.page_of(level, page, true).ok_or_else(|| std::io::Error::other("waveform overview page not readable"))?;
            buf.clear();
            p.0.iter().for_each(|b| b.write_le(&mut buf));
            w.write_all(&buf)?;
        }
        Ok(())
    }

    /// Writes [`Peaks::to_bytes`]: the sample count, the level-0 block count and level 0.
    pub fn write_level0(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        w.write_all(&(self.len as u64).to_le_bytes())?;
        w.write_all(&(self.level_len(0) as u64).to_le_bytes())?;
        self.write_level(0, w)
    }

    /// The overview as bytes (for a waveform cache file): the sample count, then each level-0
    /// block's min, max and sum of squares (little-endian `f32`). The levels above are cheap to
    /// make again and are not stored here. (A paged overview whose pages can't be read gives
    /// fewer bytes, which [`Peaks::from_bytes`] refuses.)
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.level_len(0) * BLOCK_BYTES);
        let _ = self.write_level0(&mut out);
        out
    }

    /// An overview read back from [`Peaks::to_bytes`] at the start of `bytes`, and how many bytes
    /// it took; `None` when they don't hold one (truncated, or a block count that doesn't fit the
    /// sample count).
    pub fn from_bytes(bytes: &[u8]) -> Option<(Peaks, usize)> {
        let word = |at: usize| -> Option<u64> { Some(u64::from_le_bytes(bytes.get(at..at.checked_add(8)?)?.try_into().ok()?)) };
        let len = usize::try_from(word(0)?).ok()?;
        let n = usize::try_from(word(8)?).ok()?;
        if n != len.div_ceil(BLOCK) {
            return None;
        }
        let end = n.checked_mul(BLOCK_BYTES)?.checked_add(16)?;
        let body = bytes.get(16..end)?;
        let level0: Vec<Block> = body.chunks_exact(BLOCK_BYTES).map(Block::from_le).collect();
        if len == 0 {
            return Some((Peaks::default(), end));
        }
        Some((Peaks::from_level0(level0, len), end))
    }

    /// Number of samples the overview was built from.
    pub fn len_samples(&self) -> usize {
        self.len
    }

    /// Number of resolution levels (0 for an empty overview).
    pub fn num_levels(&self) -> usize {
        match &self.store {
            Store::Full(l) => l.len(),
            Store::Paged(p) => p.sizes.len(),
        }
    }

    /// Block `i` of level `lvl` (nothing past the level's end).
    fn get(&self, lvl: usize, i: usize, lk: &mut Lookup) -> Block {
        match &self.store {
            Store::Full(levels) => levels.get(lvl).and_then(|l| l.get(i)).copied().unwrap_or(Block::EMPTY),
            Store::Paged(p) => p.get(lvl, i, lk),
        }
    }

    /// (min, max) over the sample range `[start, end)`, clamped to the data. `samples` must be the
    /// data the overview was built from. Returns `(0.0, 0.0)` for an empty range. Exact: a paged
    /// overview's pages are read for it (blocking).
    pub fn minmax(&self, samples: &[f32], start: usize, end: usize) -> (f32, f32) {
        self.sum(View::whole(samples), start, end, &mut Lookup::new(true)).range()
    }

    fn sum(&self, samples: View<'_>, start: usize, end: usize, lk: &mut Lookup) -> Block {
        let end = end.min(self.len).min(samples.len);
        let (first, last) = (start.div_ceil(BLOCK), end / BLOCK);
        let raw = |a: usize, b: usize| Block::of(samples.slice(a, b));
        if first >= last {
            return raw(start, end);
        }
        raw(start, first * BLOCK).merge(raw(last * BLOCK, end)).merge(self.blocks_from(0, first, last, lk))
    }

    /// The blocks `[lo, hi)` of level `from`, summed from the coarsest blocks that fit.
    fn blocks_from(&self, from: usize, mut lo: usize, mut hi: usize, lk: &mut Lookup) -> Block {
        let n = self.num_levels();
        let mut acc = Block::EMPTY;
        for lvl in from..n {
            if lvl + 1 == n || hi.saturating_sub(lo) < 2 * FANOUT {
                for i in lo..hi {
                    acc = acc.merge(self.get(lvl, i, lk));
                }
                break;
            }
            while !lo.is_multiple_of(FANOUT) {
                acc = acc.merge(self.get(lvl, lo, lk));
                lo += 1;
            }
            while !hi.is_multiple_of(FANOUT) {
                hi -= 1;
                acc = acc.merge(self.get(lvl, hi, lk));
            }
            lo /= FANOUT;
            hi /= FANOUT;
        }
        acc
    }

    /// The level whose blocks sum up columns `step` samples wide (see [`COLUMN_SHARE`]); 0 when
    /// columns are summed exactly.
    fn query_level(&self, step: f64) -> usize {
        let n = self.num_levels();
        let (mut lvl, mut size) = (0, (BLOCK * FANOUT) as f64);
        while lvl + 1 < n && size * COLUMN_SHARE <= step {
            lvl += 1;
            size *= FANOUT as f64;
        }
        lvl
    }

    /// The whole blocks of level `lvl` that cover the span `[a, b]` (and the sample after it, which
    /// the line from `b` reaches), and how many samples they cover.
    fn rounded(&self, lvl: usize, a: f64, b: f64, lk: &mut Lookup) -> (Block, usize) {
        let size = FANOUT.checked_pow(lvl as u32).and_then(|p| p.checked_mul(BLOCK)).unwrap_or(usize::MAX);
        let (s, e) = (a.floor() as usize, (b.floor() as usize).saturating_add(2).min(self.len));
        let (lo, hi) = (s / size, e.div_ceil(size));
        let covered = hi.saturating_mul(size).min(self.len).saturating_sub(lo.saturating_mul(size));
        (self.blocks_from(lvl, lo, hi, lk), covered)
    }

    /// The spans of sample positions `[start + c * step, start + (c + 1) * step + overlap]` that
    /// reach the data, clamped to it.
    fn windows(&self, samples: View<'_>, start: f64, step: f64, overlap: f64, cols: usize) -> impl Iterator<Item = Option<(f64, f64)>> + use<> {
        let n = self.len.min(samples.len);
        let valid = n > 0 && start.is_finite() && step.is_finite() && step > 0.0 && overlap.is_finite() && overlap >= 0.0;
        let last = n.saturating_sub(1) as f64;
        (0..cols.min(MAX_COLUMNS)).map(move |c| {
            let (a, b) = (start + step * c as f64, start + step * (c + 1) as f64 + overlap);
            (valid && b >= 0.0 && a <= last).then(|| (a.max(0.0), b.min(last)))
        })
    }

    /// (min, max) of the line through `samples` over each of `cols` spans of sample positions
    /// `[start + c * step, start + (c + 1) * step + overlap]`; fractional positions interpolate
    /// between samples. Neighbouring spans share at least their boundary, so a waveform drawn from
    /// them stays connected at any zoom. `None` for spans outside the data.
    pub fn spans(&self, samples: &[f32], start: f64, step: f64, overlap: f64, cols: usize) -> Vec<Option<(f32, f32)>> {
        self.spans_view(View::whole(samples), start, step, overlap, cols, &mut Lookup::new(false))
    }

    /// [`Peaks::spans`] reading samples only from `window`, which holds positions
    /// `[window_start, window_start + window.len())` of the channel (an audio file played from
    /// disk is read around what is shown). Spans need samples only at their partial-block edges;
    /// outside the window those few are left out (invisible once a column spans many blocks).
    pub fn spans_in(&self, window: &[f32], window_start: usize, start: f64, step: f64, overlap: f64, cols: usize) -> Vec<Option<(f32, f32)>> {
        self.spans_in_checked(window, window_start, start, step, overlap, cols).0
    }

    /// [`Peaks::spans_in`], and whether every block it needed was loaded: when not (a paged
    /// overview's page is being read), coarser blocks stood in and the caller draws again soon.
    pub fn spans_in_checked(
        &self,
        window: &[f32],
        window_start: usize,
        start: f64,
        step: f64,
        overlap: f64,
        cols: usize,
    ) -> (Vec<Option<(f32, f32)>>, bool) {
        let mut lk = Lookup::new(false);
        let out = self.spans_view(View { data: window, off: window_start, len: self.len }, start, step, overlap, cols, &mut lk);
        (out, !lk.missing)
    }

    /// [`Peaks::rms_spans`] with a sample window as in [`Peaks::spans_in`].
    pub fn rms_spans_in(&self, window: &[f32], window_start: usize, start: f64, step: f64, overlap: f64, cols: usize) -> Vec<Option<f32>> {
        self.rms_spans_in_checked(window, window_start, start, step, overlap, cols).0
    }

    /// [`Peaks::rms_spans_in`], and whether every block it needed was loaded (as
    /// [`Peaks::spans_in_checked`]).
    pub fn rms_spans_in_checked(
        &self,
        window: &[f32],
        window_start: usize,
        start: f64,
        step: f64,
        overlap: f64,
        cols: usize,
    ) -> (Vec<Option<f32>>, bool) {
        let mut lk = Lookup::new(false);
        let out = self.rms_view(View { data: window, off: window_start, len: self.len }, start, step, overlap, cols, &mut lk);
        (out, !lk.missing)
    }

    fn spans_view(&self, samples: View<'_>, start: f64, step: f64, overlap: f64, cols: usize, lk: &mut Lookup) -> Vec<Option<(f32, f32)>> {
        let lvl = self.query_level(step);
        if lvl > 0 {
            return self.windows(samples, start, step, overlap, cols).map(|w| w.map(|(a, b)| self.rounded(lvl, a, b, lk).0.range())).collect();
        }
        let at = |t: f64| {
            let i = t.floor();
            let Some(a) = samples.get(i as usize) else { return Block::EMPTY };
            let b = samples.get(i as usize + 1).unwrap_or(a);
            Block::point(a + (b - a) * (t - i) as f32)
        };
        self.windows(samples, start, step, overlap, cols)
            .map(|w| w.map(|(a, b)| self.sum(samples, a.ceil() as usize, b.floor() as usize + 1, lk).merge(at(a)).merge(at(b)).range()))
            .collect()
    }

    /// The RMS of `samples` over each span [`Peaks::spans`] gives, including the samples just
    /// outside a span's ends (zoomed out: over the whole blocks that cover it).
    pub fn rms_spans(&self, samples: &[f32], start: f64, step: f64, overlap: f64, cols: usize) -> Vec<Option<f32>> {
        self.rms_view(View::whole(samples), start, step, overlap, cols, &mut Lookup::new(false))
    }

    fn rms_view(&self, samples: View<'_>, start: f64, step: f64, overlap: f64, cols: usize, lk: &mut Lookup) -> Vec<Option<f32>> {
        let lvl = self.query_level(step);
        self.windows(samples, start, step, overlap, cols)
            .map(|w| {
                w.map(|(a, b)| {
                    if lvl > 0 {
                        let (blk, n) = self.rounded(lvl, a, b, lk);
                        return (blk.squares / n.max(1) as f32).sqrt();
                    }
                    let (s, e) = (a.floor() as usize, (b.ceil() as usize).saturating_add(1).min(self.len).min(samples.len));
                    (self.sum(samples, s, e, lk).squares / e.saturating_sub(s).max(1) as f32).sqrt()
                })
            })
            .collect()
    }
}

impl Paged {
    fn get(&self, lvl: usize, i: usize, lk: &mut Lookup) -> Block {
        if i >= self.sizes.get(lvl).copied().unwrap_or(0) {
            return Block::EMPTY;
        }
        if lvl >= FINE_LEVELS {
            return self.coarse.get(lvl - FINE_LEVELS).and_then(|l| l.get(i)).copied().unwrap_or(Block::EMPTY);
        }
        if let Some(p) = self.page(lvl, i / PAGE_BLOCKS, lk, true) {
            return p.0.get(i % PAGE_BLOCKS).copied().unwrap_or(Block::EMPTY);
        }
        let page = i / PAGE_BLOCKS;
        let gone = match lk.gone.get(lvl).copied().flatten() {
            Some((p, g)) if p == page => g,
            _ => {
                let g = self.fine.unavailable(lvl, page);
                if let Some(slot) = lk.gone.get_mut(lvl) {
                    *slot = Some((page, g));
                }
                g
            }
        };
        lk.missing |= !gone;
        // Not loaded yet: the finest coarser block at hand stands in (its sum of squares shared out).
        for m in lvl + 1..self.sizes.len() {
            let Some(div) = FANOUT.checked_pow((m - lvl) as u32) else { break };
            let j = i / div;
            let b = if m >= FINE_LEVELS {
                self.coarse.get(m - FINE_LEVELS).and_then(|l| l.get(j)).copied()
            } else {
                self.page(m, j / PAGE_BLOCKS, lk, false).and_then(|p| p.0.get(j % PAGE_BLOCKS).copied())
            };
            if let Some(b) = b {
                return Block { squares: b.squares / div as f32, ..b };
            }
        }
        Block::EMPTY
    }

    /// Page `page` of fine level `lvl` as this query has it; with `ask`, looked up (and asked
    /// for) when it hasn't yet.
    fn page(&self, lvl: usize, page: usize, lk: &mut Lookup, ask: bool) -> Option<Arc<PeakPage>> {
        let wait = lk.wait;
        let slots = lk.pages.get_mut(lvl)?;
        if let Some((_, p)) = slots.iter().flatten().find(|(k, _)| *k == page) {
            return p.clone();
        }
        if !ask {
            return None;
        }
        let p = self.fine.page(lvl, page, wait);
        slots[1] = slots[0].take();
        slots[0] = Some((page, p.clone()));
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_builder_fed_in_pieces_makes_the_same_overview_and_windows_read_the_same_spans() {
        let x: Vec<f32> = (0..100_003).map(|i| ((i as f32) * 0.0137).sin() * (1.0 + (i % 7) as f32 * 0.01)).collect();
        let whole = Peaks::build(&x);
        let mut b = PeaksBuilder::new();
        let mut at = 0;
        for n in [1usize, 31, 33, 4096, 77, 50_000].iter().cycle() {
            if at >= x.len() {
                break;
            }
            let end = (at + n).min(x.len());
            b.push(&x[at..end]);
            at = end;
        }
        assert_eq!(b.finish(), whole);
        // A window around what is shown reads the spans the whole channel does.
        let (start, step, cols) = (40_000.5, 3.7, 300);
        let (w0, w1) = (39_000usize, 42_000usize);
        assert_eq!(whole.spans_in(&x[w0..w1], w0, start, step, 0.0, cols), whole.spans(&x, start, step, 0.0, cols));
        assert_eq!(whole.rms_spans_in(&x[w0..w1], w0, start, step, 0.0, cols), whole.rms_spans(&x, start, step, 0.0, cols));
    }

    fn brute(samples: &[f32], start: usize, end: usize) -> (f32, f32) {
        let s = &samples[start..end];
        (s.iter().copied().fold(f32::INFINITY, f32::min), s.iter().copied().fold(f32::NEG_INFINITY, f32::max))
    }

    fn signal(n: usize) -> Vec<f32> {
        (0..n).map(|i| ((i as f32 * 0.037).sin() * (i as f32 * 0.0011).cos()) + if i % 997 == 0 { 0.5 } else { 0.0 }).collect()
    }

    #[test]
    fn every_range_is_exact() {
        let x = signal(100_003);
        let p = Peaks::build(&x);
        for &(a, b) in &[(0, 32), (0, 100_003), (32, 64_000), (3200, 3232), (96, 99_968), (64, 100_003), (5, 6), (31, 33), (1000, 1031)] {
            assert_eq!(p.minmax(&x, a, b), brute(&x, a, b), "range {a}..{b}");
        }
        let mut seed = 12345u64;
        for _ in 0..2000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let a = (seed >> 33) as usize % 100_003;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let b = a + 1 + (seed >> 33) as usize % (100_003 - a);
            assert_eq!(p.minmax(&x, a, b), brute(&x, a, b), "range {a}..{b}");
        }
    }

    #[test]
    fn bytes_round_trip_and_reject_garbage() {
        for n in [0usize, 1, 31, 32, 33, 100_003] {
            let x = signal(n);
            let p = Peaks::build(&x);
            let mut b = p.to_bytes();
            b.extend_from_slice(b"tail");
            let (q, used) = Peaks::from_bytes(&b).unwrap();
            assert_eq!(q, p, "{n} samples");
            assert_eq!(used, b.len() - 4);
        }
        let b = Peaks::build(&signal(1000)).to_bytes();
        assert!(Peaks::from_bytes(&b[..b.len() - 1]).is_none());
        assert!(Peaks::from_bytes(&[0xff; 16]).is_none());
        assert!(Peaks::from_bytes(&[]).is_none());
    }

    #[test]
    fn degenerate_inputs() {
        let p = Peaks::build(&[]);
        assert_eq!(p.len_samples(), 0);
        assert_eq!(p.minmax(&[], 0, 10), (0.0, 0.0));
        assert_eq!(p.spans(&[], 0.0, 1.0, 0.0, 4), vec![None; 4]);
        let x = [0.5, -0.25];
        let p = Peaks::build(&x);
        assert_eq!(p.minmax(&x, 0, 2), (-0.25, 0.5));
        assert_eq!(p.minmax(&x, 5, 3), (0.0, 0.0));
        assert_eq!(p.minmax(&x, 0, usize::MAX), (-0.25, 0.5));
        assert_eq!(p.minmax(&x[..1], 0, 2), (0.5, 0.5));
        for (start, step, overlap) in [
            (f64::NAN, 1.0, 0.0),
            (0.0, 0.0, 0.0),
            (0.0, -1.0, 0.0),
            (0.0, f64::INFINITY, 0.0),
            (f64::NEG_INFINITY, 1.0, 0.0),
            (0.0, 1.0, -0.5),
            (0.0, 1.0, f64::NAN),
        ] {
            assert_eq!(p.spans(&x, start, step, overlap, 3), vec![None; 3]);
        }
        assert_eq!(p.spans(&x, 0.0, 1.0, 0.0, 0).len(), 0);
        assert_eq!(p.spans(&x, 0.0, 1.0, 0.0, usize::MAX).len(), MAX_COLUMNS);
        let x = [f32::NAN, 1.0];
        let p = Peaks::build(&x);
        assert_eq!(p.minmax(&x, 0, 2), (1.0, 1.0));
        assert_eq!(p.spans(&x, 0.0, 1.0, 0.0, 1), vec![Some((1.0, 1.0))]);
        assert!(p.rms_spans(&x, 0.0, 1.0, 0.0, 1)[0].is_some_and(f32::is_finite));
    }

    #[test]
    fn rms_spans_match_the_samples() {
        let x = signal(20_000);
        let p = Peaks::build(&x);
        let rms = |s: usize, e: usize| (x[s..e].iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / (e - s) as f64).sqrt() as f32;
        for (step, overlap) in [(1.5, 0.0), (37.0, 37.0), (1000.0, 0.0), (4096.0, 512.0)] {
            let cols = ((19_999.0 - overlap) / step) as usize;
            // Zoomed out (a column of at least 8 blocks of 128 samples), a column sums up the whole
            // blocks of the finest such level that cover it.
            let size = [512usize, 128, 1].into_iter().find(|s| *s > 1 && (*s * 8) as f64 <= step).unwrap_or(1);
            for (c, got) in p.rms_spans(&x, 0.0, step, overlap, cols).into_iter().enumerate() {
                let (a, b) = (c as f64 * step, ((c + 1) as f64 * step + overlap).min(19_999.0));
                let (s, e) = if size > 1 {
                    (a.floor() as usize / size * size, ((b.floor() as usize + 2).min(20_000).div_ceil(size) * size).min(20_000))
                } else {
                    (a.floor() as usize, (b.ceil() as usize + 1).min(20_000))
                };
                let (got, want) = (got.unwrap(), rms(s, e));
                assert!((got - want).abs() <= want * 1e-4 + 1e-6, "span {c} at step {step}: {got} vs {want}");
            }
        }
        let sine: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let p = Peaks::build(&sine);
        let r = p.rms_spans(&sine, 0.0, 47_000.0, 0.0, 1)[0].unwrap();
        assert!((r - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{r}");
        assert_eq!(p.rms_spans(&sine, f64::NAN, 1.0, 0.0, 2), vec![None, None]);
        assert_eq!(p.rms_spans(&sine, 50_000.0, 1.0, 0.0, 1), vec![None]);
    }

    #[test]
    fn spans_cover_their_samples_and_touch_their_neighbours() {
        let x = signal(200_000);
        let p = Peaks::build(&x);
        // Up to 1024 samples a column they are exact; beyond, a little wider, never narrower.
        for step in [0.3, 1.0, 7.5, 100.0, 333.3, 1500.0, 5000.0, 40_000.0] {
            let cols = (199_999.0 / step) as usize;
            let spans = p.spans(&x, 0.0, step, 0.0, cols);
            for (c, s) in spans.iter().enumerate() {
                let (lo, hi) = s.unwrap_or_else(|| panic!("span {c} at step {step}"));
                let (a, b) = ((c as f64 * step).ceil() as usize, ((c + 1) as f64 * step).floor() as usize);
                if a <= b {
                    let truth = brute(&x, a, b + 1);
                    assert!(lo <= truth.0 && hi >= truth.1, "span {c} at step {step}");
                    if step >= 1024.0 {
                        let size = if step >= 65_536.0 {
                            8192
                        } else if step >= 16_384.0 {
                            2048
                        } else if step >= 4096.0 {
                            512
                        } else {
                            128
                        };
                        let wide = brute(&x, a / size * size, ((b + 2).div_ceil(size) * size).min(200_000));
                        assert_eq!((lo, hi), wide, "span {c} at step {step}");
                    }
                }
                if let Some(Some((nlo, nhi))) = spans.get(c + 1) {
                    assert!(*nlo <= hi && *nhi >= lo, "spans {c} and {} at step {step} don't touch", c + 1);
                }
            }
            let all = spans.iter().flatten().fold((f32::INFINITY, f32::NEG_INFINITY), |a, s| (a.0.min(s.0), a.1.max(s.1)));
            if step < 1024.0 {
                assert_eq!(all, brute(&x, 0, ((cols as f64 * step).floor() as usize + 1).min(200_000)), "step {step}");
            }
        }
    }

    #[test]
    fn spans_follow_the_line_between_samples() {
        let x = [0.0, 1.0, -1.0, 0.5];
        let p = Peaks::build(&x);
        assert_eq!(p.spans(&x, 0.0, 0.25, 0.0, 4), vec![Some((0.0, 0.25)), Some((0.25, 0.5)), Some((0.5, 0.75)), Some((0.75, 1.0))]);
        assert_eq!(p.spans(&x, 0.5, 1.0, 0.0, 1), vec![Some((0.0, 1.0))]);
        let spans = vec![None, Some((0.0, 0.0)), Some((0.0, 1.0)), Some((-1.0, 1.0)), Some((-1.0, 0.5)), Some((0.5, 0.5)), None];
        assert_eq!(p.spans(&x, -2.0, 1.0, 0.0, 7), spans);
        assert_eq!(p.spans(&x, 0.0, 1.0, 1.0, 3), vec![Some((-1.0, 1.0)), Some((-1.0, 1.0)), Some((-1.0, 0.5))]);
        assert_eq!(p.spans(&x, 0.0, 1.0, 0.5, 3), vec![Some((0.0, 1.0)), Some((-1.0, 1.0)), Some((-1.0, 0.5))]);
    }

    /// Fine levels served from a whole overview, a page at a time, counting reads; pages listed in
    /// `held` are "loaded", others are asked for (and then loaded, as a worker would).
    #[derive(Debug)]
    struct Served {
        whole: Peaks,
        held: std::sync::Mutex<std::collections::HashSet<(usize, usize)>>,
        asked: std::sync::atomic::AtomicUsize,
    }

    impl FineLevels for Served {
        fn page(&self, level: usize, page: usize, wait: bool) -> Option<Arc<PeakPage>> {
            let mut held = self.held.lock().unwrap();
            if !wait && !held.contains(&(level, page)) {
                held.insert((level, page));
                self.asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return None;
            }
            self.whole.page_of(level, page, true)
        }
    }

    fn paged_of(whole: &Peaks) -> (Peaks, Arc<Served>) {
        let mut b = LevelsBuilder::new(whole.len_samples());
        let mut bytes = Vec::new();
        whole.write_level(0, &mut bytes).unwrap();
        b.push_level0(&bytes);
        let upper = b.finish().unwrap();
        let served = Arc::new(Served { whole: whole.clone(), held: Default::default(), asked: Default::default() });
        (upper.into_paged(served.clone()).unwrap(), served)
    }

    #[test]
    fn a_paged_overview_answers_as_the_whole_one_once_its_pages_are_in() {
        let x = signal(3_000_017);
        let whole = Peaks::build(&x);
        let (paged, served) = paged_of(&whole);
        assert!(paged.is_paged() && !whole.is_paged());
        assert_eq!(paged, whole, "the levels built from level 0 bytes are the overview's");
        assert!(paged.memory_bytes() * 200 < whole.memory_bytes(), "{} vs {}", paged.memory_bytes(), whole.memory_bytes());
        assert_eq!(paged.minmax(&x, 1_234, 2_345_678), whole.minmax(&x, 1_234, 2_345_678));
        for (start, step, overlap, cols) in [
            (0.0, 1.7, 0.0, 800),
            (1_000_000.3, 40.0, 40.0, 2000),
            (500.0, 900.0, 0.0, 2000),
            (0.0, 3000.0, 3000.0, 999),
            (7.0, 2_000.0 * 8.0 * 4.0, 0.0, 47),
        ] {
            let first = (start as usize).saturating_sub(300);
            let window = &x[first..(first + (step * cols as f64) as usize + 900).min(x.len())];
            let want = (whole.spans_in(window, first, start, step, overlap, cols), whole.rms_spans_in(window, first, start, step, overlap, cols));
            // At first some pages are missing: coarser blocks stand in, never a gap, never narrower.
            let (rough, done) = paged.spans_in_checked(window, first, start, step, overlap, cols);
            let mut tries = 0;
            let (mut got, mut complete) = (rough.clone(), done);
            while !complete {
                tries += 1;
                assert!(tries < 4, "pages keep missing");
                (got, complete) = paged.spans_in_checked(window, first, start, step, overlap, cols);
            }
            assert_eq!(got, want.0, "step {step}");
            for (r, w) in rough.iter().zip(&want.0) {
                let (Some(r), Some(w)) = (r, w) else { panic!("a gap") };
                assert!(r.0 <= w.0 && r.1 >= w.1, "the stand-in is narrower");
            }
            let mut rms = paged.rms_spans_in_checked(window, first, start, step, overlap, cols);
            for _ in 0..3 {
                if rms.1 {
                    break;
                }
                rms = paged.rms_spans_in_checked(window, first, start, step, overlap, cols);
            }
            assert!(rms.1, "pages keep missing");
            assert_eq!(rms.0, want.1, "step {step}");
        }
        // The pages read for a screenful are few, at any zoom.
        let asked = served.asked.load(std::sync::atomic::Ordering::Relaxed);
        assert!(asked < 200, "{asked} pages asked for");
        // Written out it is the same bytes.
        for l in 0..whole.num_levels() {
            let (mut a, mut b) = (Vec::new(), Vec::new());
            whole.write_level(l, &mut a).unwrap();
            paged.write_level(l, &mut b).unwrap();
            assert!(a == b, "level {l}");
        }
        assert!(paged.to_bytes() == whole.to_bytes());
    }

    #[test]
    fn zoomed_all_the_way_out_a_paged_overview_reads_no_fine_page() {
        let x = signal(4_000_000);
        let whole = Peaks::build(&x);
        let (paged, served) = paged_of(&whole);
        // Columns of 8 blocks of level 4 or more: the levels in memory alone sum them.
        let step = 8.0 * 8192.0 * 2.0;
        let (got, done) = paged.spans_in_checked(&[], 0, 0.0, step, 0.0, 30);
        assert!(done && served.asked.load(std::sync::atomic::Ordering::Relaxed) == 0);
        assert_eq!(got, whole.spans_in(&[], 0, 0.0, step, 0.0, 30));
        // Fine pages that can never be read: the coarse levels draw it, with no gap, for good.
        #[derive(Debug)]
        struct Gone;
        impl FineLevels for Gone {
            fn page(&self, _: usize, _: usize, _: bool) -> Option<Arc<PeakPage>> {
                None
            }
            fn unavailable(&self, _: usize, _: usize) -> bool {
                true
            }
        }
        let mut b = LevelsBuilder::new(x.len());
        let mut bytes = Vec::new();
        whole.write_level(0, &mut bytes).unwrap();
        b.push_level0(&bytes);
        let gone = b.finish().unwrap().into_paged(Arc::new(Gone)).unwrap();
        let (spans, done) = gone.spans_in_checked(&[], 0, 1_000.0, 500.0, 0.0, 2000);
        assert!(done, "nothing left to wait for");
        let truth = whole.spans_in(&[], 0, 1_000.0, 500.0, 0.0, 2000);
        for (g, t) in spans.iter().zip(&truth) {
            let (Some(g), Some(t)) = (g, t) else { panic!("a gap") };
            assert!(g.0 <= t.0 && g.1 >= t.1);
        }
        assert!(LevelsBuilder::new(100).finish().is_none(), "no blocks given");
        assert!(Peaks::paged(100, Vec::new(), served).is_none(), "too short to page");
    }
}
