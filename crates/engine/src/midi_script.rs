//! Script MIDI effects (`soundcraft_mix::midi_fx::script`): each track's script texts
//! (`Track::midi_scripts`) are compiled here, off the audio thread, after a command or an undo
//! step and when a session is opened, into the script's runtime, which the mix hands to the
//! effect. A text compiles once: a copy of a track gets a compiled script of its own (each runs
//! with its own state), and a text that does not compile keeps its error for the editor (the
//! effect then passes its events).

use soundcraft_model::{MidiScript, Session};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// The fingerprint of a script's text.
pub fn fingerprint(source: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut h);
    h.finish()
}

fn program_ptr(sc: &MidiScript) -> Option<usize> {
    sc.runtime.program.as_ref().map(|p| Arc::as_ptr(p) as *const () as usize)
}

/// Whether some script's compiled form is missing, out of date or shared with another.
pub fn any_stale(s: &Session) -> bool {
    let mut seen: Vec<usize> = Vec::new();
    for sc in s.tracks.iter().flat_map(|t| t.midi_scripts.iter()) {
        if sc.runtime.made_from != Some(fingerprint(&sc.source)) {
            return true;
        }
        if let Some(p) = program_ptr(sc) {
            if seen.contains(&p) {
                return true;
            }
            seen.push(p);
        }
    }
    false
}

/// Compiles `sc`'s text into its runtime (the program, or the error).
pub fn compile(sc: &mut MidiScript) {
    sc.runtime.made_from = Some(fingerprint(&sc.source));
    match soundcraft_mix::midi_fx::script::compile(&sc.source) {
        Ok(p) => {
            sc.runtime.program = Some(p);
            sc.runtime.error = None;
        }
        Err(e) => {
            sc.runtime.program = None;
            sc.runtime.error = Some(e);
        }
    }
}

/// Compiles every stale script (see [`any_stale`]).
pub fn refresh(s: &mut Session) {
    let mut seen: Vec<usize> = Vec::new();
    for sc in s.tracks.iter_mut().flat_map(|t| t.midi_scripts.iter_mut()) {
        let shared = program_ptr(sc).is_some_and(|p| seen.contains(&p));
        if shared || sc.runtime.made_from != Some(fingerprint(&sc.source)) {
            compile(sc);
        }
        if let Some(p) = program_ptr(sc) {
            seen.push(p);
        }
    }
}
