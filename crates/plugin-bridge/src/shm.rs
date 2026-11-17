//! The shared memory both processes map: a file in the temp folder seen as `u32` words, every one
//! of them an atomic. The only `unsafe` code of the crate.
//!
//! Every word is only ever read and written through `AtomicU32`, by both processes, so no access
//! is a data race whatever the other process does: a misbehaving helper can at worst write
//! nonsense values, which the host treats as hostile input (it clamps, checks and bounds them).

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;

/// One mapping of the exchange file.
pub struct Shm {
    map: memmap2::MmapMut,
    words: usize,
    /// The file, removed when the side that made it lets go.
    path: PathBuf,
    owner: bool,
}

impl Shm {
    /// Makes the file at `path`, `words` long (zeroed), and maps it.
    pub fn create(path: &Path, words: usize) -> std::io::Result<Shm> {
        let bytes = words.checked_mul(4).ok_or_else(|| std::io::Error::other("too large"))?;
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(path)?;
        file.set_len(bytes as u64)?;
        Shm::map(&file, path, words, true)
    }

    /// Maps the file another process made, which must hold at least `words` words.
    pub fn open(path: &Path, words: usize) -> std::io::Result<Shm> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len();
        if len < (words as u64).saturating_mul(4) {
            return Err(std::io::Error::other("the shared file is too small"));
        }
        Shm::map(&file, path, words, false)
    }

    fn map(file: &File, path: &Path, words: usize, owner: bool) -> std::io::Result<Shm> {
        // SAFETY: memmap2 asks that nothing else changes the file while the map is read through
        // plain references. This map is only ever read through `AtomicU32` (see `words`), so a
        // concurrent change by the other process (the only other user of this private temp file)
        // is an atomic access racing an atomic access, which is defined.
        let map = unsafe { memmap2::MmapOptions::new().len(words.saturating_mul(4)).map_mut(file)? };
        Ok(Shm { map, words, path: path.to_path_buf(), owner })
    }

    /// The mapping as atomic words.
    pub fn words(&self) -> &[AtomicU32] {
        let ptr = self.map.as_ptr();
        if ptr.align_offset(std::mem::align_of::<AtomicU32>()) != 0 || self.map.len() < self.words.saturating_mul(4) {
            return &[];
        }
        // SAFETY: the mapping is page-aligned (checked above for `AtomicU32`'s alignment) and at
        // least `words * 4` bytes long (checked), lives as long as `self` (the borrow), and every
        // bit pattern is a valid `AtomicU32`, which has the size of a `u32`. All access goes
        // through these atomics, so shared mutation through `&` is sound.
        unsafe { std::slice::from_raw_parts(ptr.cast::<AtomicU32>(), self.words) }
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        if self.owner {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
