//! Clips played through an ARA plug-in (see `soundcraft_engine::ara`).
//!
//! Realtime: the host's model thread (the UI) makes one ARA playback renderer instance per clip
//! (an [`AraVoice`]: bound to the plug-in's ARA document, given the clip's playback region,
//! prepared) and puts it on the engine's [`AraBoard`]. The audio thread copies the board's voices
//! into a list it owns whenever the board changed and its lock is free (`try_lock`: it never
//! waits), and for each clip with a voice runs the instance for the block, read ahead by the
//! plug-in's latency, the transport at the read position, in place of reading the clip's file;
//! clip gain, fades and crossfades are applied to what it renders as to the file's audio. A
//! voice's instance is only ever taken out, made or dropped on the model thread, which takes the
//! voice off the board first. A plug-in that renders something that is not a number is bypassed
//! for good (the clip plays its own audio) and says so ([`AraVoice::failed`]).
//!
//! Offline (bounce, freeze, commit, consolidate): the model thread renders each such clip
//! through an instance set up for offline rendering into [`soundcraft_model::AraRender`]s on the
//! session, and every engine plays a clip's render in place of its file while it still fits the
//! clip, so an offline render hears exactly what playback did.

use soundcraft_ara_host::InstanceCell;
use soundcraft_dsp::Plugin;
use soundcraft_model::{Clip, Session};
use soundcraft_time::Samples;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Most voices the audio thread follows (more clips play their own audio).
pub const MAX_VOICES: usize = 1024;

/// One clip's ARA playback renderer instance, shared between the model thread (which made it and
/// destroys it, see `soundcraft_ara_host::BoundInstance`) and the audio thread.
pub struct AraVoice {
    cell: Arc<InstanceCell<dyn Plugin>>,
    latency: AtomicUsize,
    failed: AtomicBool,
}

impl AraVoice {
    /// A voice playing the instance in `cell` (bound, given its region and prepared on the model
    /// thread), whose latency is `latency` samples.
    pub fn new(cell: Arc<InstanceCell<dyn Plugin>>, latency: usize) -> AraVoice {
        AraVoice { cell, latency: AtomicUsize::new(latency), failed: AtomicBool::new(false) }
    }

    /// Whether the plug-in rendered something that is not a number (it is then bypassed).
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Its latency (samples) when it was made.
    pub fn latency(&self) -> usize {
        self.latency.load(Ordering::Relaxed)
    }

    /// Renders the block at timeline `at` (`frames` long) into `out` (cleared first), the
    /// instance read ahead by its latency. `false` (and `out` untouched) when it cannot now: the
    /// model thread holds it, it was taken out, or it failed. Never blocks or allocates.
    pub(crate) fn render(&self, s: &Session, at: Samples, frames: usize, looping: Option<(f64, f64)>, playing: bool, out: &mut [Vec<f32>]) -> bool {
        if self.failed() || out.iter().any(|c| c.len() < frames) {
            return false;
        }
        let read = at.saturating_add(self.latency() as i64);
        let ran = self.cell.try_with(|p| {
            for c in out.iter_mut() {
                c.iter_mut().take(frames).for_each(|x| *x = 0.0);
            }
            p.set_transport(&crate::transport_at(s, read, playing, false));
            p.set_loop(looping);
            p.process(out, frames);
        });
        if ran.is_none() {
            return false;
        }
        let ok = out.iter().all(|c| c.iter().take(frames).all(|x| x.is_finite()));
        if !ok {
            self.failed.store(true, Ordering::Relaxed);
        }
        ok
    }
}

/// The voices of a realtime engine, by clip id: written on the model thread, read (without
/// waiting) on the audio thread.
#[derive(Default)]
pub struct AraBoard {
    voices: Mutex<HashMap<u64, Arc<AraVoice>>>,
    generation: AtomicU64,
    preview: AtomicBool,
}

impl AraBoard {
    /// Whether the voices render while the transport is stopped too: an ARA plug-in's editor is
    /// open and may preview what is edited (ARA's editor renderer role).
    pub fn set_preview(&self, on: bool) {
        self.preview.store(on, Ordering::Relaxed);
    }

    pub fn preview(&self) -> bool {
        self.preview.load(Ordering::Relaxed)
    }

    /// Puts clip `clip`'s voice on the board (model thread), in place of one it had (returned).
    pub fn set(&self, clip: u64, voice: Arc<AraVoice>) -> Option<Arc<AraVoice>> {
        let old = self.voices.lock().unwrap_or_else(PoisonError::into_inner).insert(clip, voice);
        self.generation.fetch_add(1, Ordering::Release);
        old
    }

    /// Takes clip `clip`'s voice off the board (model thread). The audio thread may still hold it
    /// for the block it renders: [`AraVoice::take`] waits for that.
    pub fn remove(&self, clip: u64) -> Option<Arc<AraVoice>> {
        let old = self.voices.lock().unwrap_or_else(PoisonError::into_inner).remove(&clip);
        self.generation.fetch_add(1, Ordering::Release);
        old
    }

    /// The clips that have a voice.
    pub fn clips(&self) -> Vec<u64> {
        self.voices.lock().unwrap_or_else(PoisonError::into_inner).keys().copied().collect()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Copies the voices into `into` (its capacity reserved: never grows), without waiting: `false`
    /// when the model thread holds the board.
    pub(crate) fn copy_into(&self, into: &mut Vec<(u64, Arc<AraVoice>)>) -> bool {
        let Ok(v) = self.voices.try_lock() else { return false };
        into.clear();
        for (k, voice) in v.iter() {
            if into.len() >= into.capacity() {
                break;
            }
            into.push((*k, voice.clone()));
        }
        true
    }
}

/// The audio thread's copy of the board.
pub(crate) struct AraCache {
    pub board: Arc<AraBoard>,
    pub voices: Vec<(u64, Arc<AraVoice>)>,
    pub seen: u64,
}

impl AraCache {
    pub fn new(board: Arc<AraBoard>) -> AraCache {
        AraCache { board, voices: Vec::with_capacity(MAX_VOICES), seen: u64::MAX }
    }

    /// Follows the board when it changed (never waits; tries again next block when busy).
    pub fn refresh(&mut self) {
        let g = self.board.generation();
        if g != self.seen && self.board.copy_into(&mut self.voices) {
            self.seen = g;
        }
    }
}

/// The voice of `clip`, if the engine has one.
pub(crate) fn voice<'a>(voices: &'a [(u64, Arc<AraVoice>)], clip: &Clip) -> Option<&'a AraVoice> {
    voices.iter().find(|(k, _)| *k == clip.id.0).map(|(_, v)| &**v)
}

/// The offline render of `clip` that still fits it, if any (see [`soundcraft_model::AraRender`]).
pub fn render_of<'a>(s: &'a Session, clip: &Clip) -> Option<&'a soundcraft_model::AraRender> {
    if s.ara_renders.0.is_empty() {
        return None;
    }
    s.ara_renders.0.get(&clip.id).map(|r| &**r).filter(|r| r.serves(clip))
}
