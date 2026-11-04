//! Command ids of the vocal tuner, for the UI to call through (the Vocal Editor and its panels).
//! One const per command, named after the id.

/// Track › Vocal Track: the Vocal flag (`{tracks?, value?: bool, auto?: true}`).
pub const TRACK_VOCAL: &str = "track.vocal";

/// Tune Vocal: set Centre, Drift, Raise, Lower, Vibrato, Snap to key or Transition on notes
/// (`{notes: [{clip, note}], centre?…, reference?: selection|track}`); a slider drag merges.
pub const VOCAL_EDIT: &str = "vocal.edit";

/// Tune Vocal: apply a macro preset or values as one edit (`{notes, preset?, centre?…}`).
pub const VOCAL_MACRO: &str = "vocal.macro";

/// Mixer strip Centre note / Flatten drift step on the note under a time (`{track, at, kind}`).
pub const VOCAL_STEP: &str = "vocal.step";

/// Tune Vocal: reset notes' edits (`{notes}`).
pub const VOCAL_RESET: &str = "vocal.reset";

/// Tune Vocal: split a note at a time (`{clip, note, at}`).
pub const VOCAL_SPLIT: &str = "vocal.split";

/// Tune Vocal: join two neighbouring notes (`{clip, first, second}`).
pub const VOCAL_JOIN: &str = "vocal.join";

/// Tune Vocal: the notes, unvoiced regions and summary to draw (`{clip? | track?, from?, to?}`; a query).
pub const VOCAL_NOTES: &str = "vocal.notes";

/// The Tuning panel's Notes selector: how clips' pitch is cut into notes
/// (`{clip? | clips? | track?, mode: classic|musical}`).
pub const VOCAL_NOTE_MODE: &str = "vocal.note_mode";

/// AI › Tune Vocal › Tuned / Original: flip the A/B switch (`{clip? | clips? | track?}`).
pub const VOCAL_TOGGLE: &str = "vocal.toggle";

/// AI › Tune Vocal › Commit: bake the tuning (and Clean Vocal) into a new source (`{clip? | clips?}`).
pub const VOCAL_COMMIT: &str = "vocal.commit";

/// AI › Tune Vocal › Remove Tuning: drop a clip's tuning (`{clip? | clips?}`).
pub const VOCAL_REMOVE: &str = "vocal.remove";
