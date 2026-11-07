//! Juke patterns: step patterns as clips on instrument and MIDI tracks (`docs/juke.md`).
//!
//! A pattern clip is a MIDI clip whose notes are its pattern's expansion. These commands edit the
//! pattern; [`crate::pattern::refresh`] (run after every command, in the same undo step) expands
//! it into the clip's notes. Steps are numbered from 0, lanes from 0.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::pattern::{Condition, Direction, Lane, MAX_LANES, MAX_ROLL, MAX_STEPS, Pattern, Rate, STEPS_PER_BAR, Step};
use soundcraft_model::{Clip, PatternClip, TrackKind};

/// The lanes a new pattern starts with: a General MIDI drum kit's core, which Juke's default kit,
/// the Drum Synth and most drum instruments play.
pub(crate) const DEFAULT_LANES: [u8; 8] = [36, 38, 42, 46, 39, 41, 47, 49];
/// Most bars a new pattern clip spans.
const MAX_NEW_BARS: i64 = 64;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "pattern.new",
            "New Pattern",
            [],
            None,
            "{track?, start? (default: the selection's start; snapped back to its bar), bars?: 1-64 (default 1), steps?: 8|12|16|24|32 (default 16), lanes?: [note]} -> {clip, start, length}: a pattern clip on an instrument or MIDI track, its lanes General MIDI drums unless given",
            always,
            new_pattern
        ),
        cmd!(query "pattern.get", "Get Pattern", [], None, "{clip} -> {pattern, phase}", always, get),
        cmd!(
            "pattern.step",
            "Edit Step",
            [],
            None,
            "{clip, lane, step, on?, toggle?, velocity?: 1-127, accent?, probability?: 0-100, offset?: -50-50, roll?: 1-4, flam?, condition?: always|first|not_first|fill|not_fill|\"n:m\"} -> {step}",
            always,
            step
        ),
        cmd!(
            "pattern.paint",
            "Paint Steps",
            [],
            None,
            "{clip, lane, from, to, on, velocity?: 1-127 (steps turned on), } -> {changed}: turns a run of steps on or off",
            always,
            paint
        ),
        cmd!(
            "pattern.lane",
            "Edit Lane",
            [],
            None,
            "{clip, lane, note?: 0-127, length?: 1-64, direction?: forward|reverse|ping_pong|random, rate?: half|normal|double, mute?} -> {lane}",
            always,
            lane
        ),
        cmd!("pattern.lane_add", "Add Lane", [], None, "{clip, note, length?} -> {lane}: a lane at the bottom", always, lane_add),
        cmd!("pattern.lane_remove", "Remove Lane", [], None, "{clip, lane}", always, lane_remove),
        cmd!(
            "pattern.settings",
            "Pattern Settings",
            [],
            None,
            "{clip, steps_per_bar?: 8|12|16|24|32, shuffle?: 0-100, accent?: 0-127, humanize?: 0-100, fill_every?: 0-16} -> {pattern}",
            always,
            settings
        ),
        cmd!("pattern.reroll", "Re-roll Pattern", [], None, "{clip} -> {seed}: draws every chance again", always, reroll),
        cmd!(
            "pattern.to_midi",
            "Convert Pattern to MIDI",
            [],
            None,
            "{clip?|clips? (default: the selected clips)} -> {clips}: pattern clips keep their notes as plain MIDI clips",
            always,
            to_midi
        ),
    ]
}

fn pattern_clip<'a>(e: &'a Engine, id: ClipId, cmd: &str) -> Result<&'a PatternClip> {
    match e.session().find_clip(id) {
        Some((_, c)) => c.pattern.as_ref().ok_or_else(|| bad(cmd, "not a pattern clip")),
        None => Err(bad(cmd, "no such clip")),
    }
}

fn pattern_mut<'a>(e: &'a mut Engine, id: ClipId, cmd: &str) -> Result<&'a mut Pattern> {
    // Check first, so a refused command does not copy the session.
    pattern_clip(e, id, cmd)?;
    match e.session_mut().find_clip_mut(id).and_then(|c| c.pattern.as_mut()) {
        Some(pc) => Ok(&mut pc.pattern),
        None => Err(bad(cmd, "not a pattern clip")),
    }
}

fn index(p: &Value, key: &str, cmd: &str) -> Result<usize> {
    p.get(key).and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(cmd, format!("`{key}` (a number from 0) required")))
}

fn lane_mut<'a>(pat: &'a mut Pattern, p: &Value, cmd: &str) -> Result<&'a mut Lane> {
    let i = index(p, "lane", cmd)?;
    let n = pat.lanes.len();
    pat.lanes.get_mut(i).ok_or_else(|| bad(cmd, format!("lane {i} is out of range (the pattern has {n})")))
}

fn ranged(p: &Value, key: &str, lo: i64, hi: i64, cmd: &str) -> Result<Option<i64>> {
    match p.get(key) {
        None => Ok(None),
        Some(v) => match v.as_i64() {
            Some(n) if (lo..=hi).contains(&n) => Ok(Some(n)),
            _ => Err(bad(cmd, format!("`{key}` must be a whole number from {lo} to {hi}"))),
        },
    }
}

fn flag(p: &Value, key: &str, cmd: &str) -> Result<Option<bool>> {
    match p.get(key) {
        None => Ok(None),
        Some(v) => v.as_bool().map(Some).ok_or_else(|| bad(cmd, format!("`{key}` must be true or false"))),
    }
}

/// `always`, `first`, `not_first`, `fill`, `not_fill`, `"n:m"` (the n-th pass of every m) or the
/// stored form (`{"kind": "every", "nth": n, "of": m}`).
fn condition(v: &Value, cmd: &str) -> Result<Condition> {
    let err = || bad(cmd, "`condition` must be always, first, not_first, fill, not_fill or \"n:m\" (1 ≤ n ≤ m ≤ 16)");
    if let Some(text) = v.as_str() {
        return Ok(match text {
            "always" => Condition::Always,
            "first" => Condition::First,
            "not_first" => Condition::NotFirst,
            "fill" => Condition::Fill,
            "not_fill" => Condition::NotFill,
            _ => {
                let (a, b) = text.split_once(':').ok_or_else(err)?;
                let (nth, of) = (a.trim().parse::<u8>().map_err(|_| err())?, b.trim().parse::<u8>().map_err(|_| err())?);
                if !(1..=16).contains(&of) || !(1..=of).contains(&nth) {
                    return Err(err());
                }
                Condition::Every { nth, of }
            }
        });
    }
    serde_json::from_value(v.clone()).map_err(|_| err())
}

fn new_pattern(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "pattern.new";
    let track = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    if !matches!(e.session().track(track).map(|t| t.kind), Some(TrackKind::Instrument | TrackKind::Midi)) {
        return Err(bad(id, "patterns go on instrument and MIDI tracks"));
    }
    let bars = ranged(p, "bars", 1, MAX_NEW_BARS, id)?.unwrap_or(1);
    let steps = match ranged(p, "steps", 1, 32, id)? {
        None => 16,
        Some(n) if STEPS_PER_BAR.contains(&(n as u8)) => n as u8,
        Some(_) => return Err(bad(id, "`steps` must be 8, 12, 16, 24 or 32")),
    };
    let notes: Vec<u8> = match p.get("lanes") {
        None => DEFAULT_LANES.to_vec(),
        Some(v) => {
            let arr = v.as_array().ok_or_else(|| bad(id, "`lanes` must be a list of notes"))?;
            if arr.is_empty() || arr.len() > MAX_LANES {
                return Err(bad(id, format!("`lanes` takes 1 to {MAX_LANES} notes")));
            }
            arr.iter()
                .map(|n| n.as_u64().filter(|&n| n <= 127).map(|n| n as u8).ok_or_else(|| bad(id, "each lane is a note from 0 to 127")))
                .collect::<Result<_>>()?
        }
    };
    let at = position_param(e, id, p, "start")?.unwrap_or(e.session().edit.selection.start);
    let (clip_id, start, length) = place_pattern(e, track, at, bars, steps, &notes)?;
    Ok(json!({"clip": clip_id.0, "start": start, "length": length}))
}

/// Puts a new pattern clip of `bars` bars on `track`, from the bar `at` is in: `steps` per bar, a
/// lane per note. Returns the clip, its start and its length.
pub(crate) fn place_pattern(e: &mut Engine, track: TrackId, at: Samples, bars: i64, steps: u8, notes: &[u8]) -> Result<(ClipId, Samples, Samples)> {
    let id = "pattern.new";
    let s = e.session();
    let sr = s.sample_rate;
    // Back to the bar the position is in, so the steps sit on the grid.
    let bar = s.tempo.bar_beat_at_tick(s.tempo.samples_to_ticks(at, sr)).bar;
    let t0 = s.tempo.bar_start_tick(bar);
    let t1 = s.tempo.bar_start_tick(bar.saturating_add(bars));
    let (start, end) = (s.tempo.tick_to_samples(t0, sr), s.tempo.tick_to_samples(t1, sr));
    if end <= start {
        return Err(bad(id, "the pattern would be empty"));
    }
    let ses = e.session_mut();
    let clip_id = ses.new_clip_id();
    let pattern = Pattern {
        steps_per_bar: steps,
        lanes: notes.iter().map(|&n| Lane::new(n, usize::from(steps))).collect(),
        seed: clip_id.0.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
        ..Pattern::default()
    };
    let mut clip = Clip::midi(clip_id, "Pattern", start, end - start, soundcraft_midi::Sequence::default());
    clip.pattern = Some(PatternClip::new(pattern, 0));
    crate::edit::place_clip(ses, track, clip);
    Ok((clip_id, start, end - start))
}

fn get(e: &mut Engine, p: &Value) -> Result<Value> {
    let pc = pattern_clip(e, clip_id(p, "pattern.get")?, "pattern.get")?;
    Ok(json!({"pattern": pc.pattern, "phase": pc.phase}))
}

fn step(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "pattern.step";
    let clip = clip_id(p, id)?;
    let i = index(p, "step", id)?;
    let (on, toggle) = (flag(p, "on", id)?, flag(p, "toggle", id)?.unwrap_or(false));
    let velocity = ranged(p, "velocity", 1, 127, id)?;
    let accent = flag(p, "accent", id)?;
    let probability = ranged(p, "probability", 0, 100, id)?;
    let offset = ranged(p, "offset", -50, 50, id)?;
    let roll = ranged(p, "roll", 1, i64::from(MAX_ROLL), id)?;
    let flam = flag(p, "flam", id)?;
    let cond = p.get("condition").map(|v| condition(v, id)).transpose()?;
    let lane = lane_mut(pattern_mut(e, clip, id)?, p, id)?;
    let len = lane.steps.len();
    let s = lane.steps.get_mut(i).ok_or_else(|| bad(id, format!("step {i} is out of range (the lane has {len})")))?;
    if toggle {
        s.on = !s.on;
    }
    if let Some(v) = on {
        s.on = v;
    }
    if let Some(v) = velocity {
        s.velocity = v as u8;
    }
    if let Some(v) = accent {
        s.accent = v;
    }
    if let Some(v) = probability {
        s.probability = v as u8;
    }
    if let Some(v) = offset {
        s.offset = v as i8;
    }
    if let Some(v) = roll {
        s.roll = v as u8;
    }
    if let Some(v) = flam {
        s.flam = v;
    }
    if let Some(c) = cond {
        s.condition = c;
    }
    Ok(json!({"step": *s}))
}

fn paint(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "pattern.paint";
    let clip = clip_id(p, id)?;
    let (from, to) = (index(p, "from", id)?, index(p, "to", id)?);
    let on = flag(p, "on", id)?.ok_or_else(|| bad(id, "`on` (true or false) required"))?;
    let velocity = ranged(p, "velocity", 1, 127, id)?;
    // Count first: a paint that changes nothing leaves the session (and the undo history) alone.
    let pc = pattern_clip(e, clip, id)?;
    let lane_i = index(p, "lane", id)?;
    let lane = pc.pattern.lanes.get(lane_i).ok_or_else(|| bad(id, format!("lane {lane_i} is out of range")))?;
    let (lo, hi) = (from.min(to), from.max(to).min(lane.steps.len().saturating_sub(1)));
    if lo >= lane.steps.len() {
        return Err(bad(id, format!("step {lo} is out of range (the lane has {})", lane.steps.len())));
    }
    let changed = lane.steps.get(lo..=hi).unwrap_or_default().iter().filter(|s| s.on != on).count();
    if changed == 0 {
        return Ok(json!({"changed": 0}));
    }
    let lane = lane_mut(pattern_mut(e, clip, id)?, p, id)?;
    for s in lane.steps.get_mut(lo..=hi).unwrap_or_default() {
        if s.on != on {
            *s = if on { Step { velocity: velocity.map_or(s.velocity, |v| v as u8), ..Step::hit(s.velocity) } } else { Step { on: false, ..*s } };
        }
    }
    Ok(json!({"changed": changed}))
}

fn direction(v: &Value, id: &str) -> Result<Direction> {
    serde_json::from_value(v.clone()).map_err(|_| bad(id, "`direction` must be forward, reverse, ping_pong or random"))
}

fn rate(v: &Value, id: &str) -> Result<Rate> {
    serde_json::from_value(v.clone()).map_err(|_| bad(id, "`rate` must be half, normal or double"))
}

fn lane(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "pattern.lane";
    let clip = clip_id(p, id)?;
    let note = ranged(p, "note", 0, 127, id)?;
    let length = ranged(p, "length", 1, MAX_STEPS as i64, id)?;
    let dir = p.get("direction").map(|v| direction(v, id)).transpose()?;
    let rt = p.get("rate").map(|v| rate(v, id)).transpose()?;
    let mute = flag(p, "mute", id)?;
    let lane = lane_mut(pattern_mut(e, clip, id)?, p, id)?;
    if let Some(n) = note {
        lane.note = n as u8;
    }
    if let Some(n) = length {
        lane.steps.resize(n as usize, Step::default());
    }
    if let Some(d) = dir {
        lane.direction = d;
    }
    if let Some(r) = rt {
        lane.rate = r;
    }
    if let Some(m) = mute {
        lane.mute = m;
    }
    Ok(json!({"lane": lane}))
}

fn lane_add(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "pattern.lane_add";
    let clip = clip_id(p, id)?;
    let note = ranged(p, "note", 0, 127, id)?.ok_or_else(|| bad(id, "`note` required"))?;
    let pat = pattern_mut(e, clip, id)?;
    if pat.lanes.len() >= MAX_LANES {
        return Err(bad(id, format!("a pattern holds at most {MAX_LANES} lanes")));
    }
    let length = ranged(p, "length", 1, MAX_STEPS as i64, id)?.map_or(usize::from(pat.steps_per_bar), |n| n as usize);
    pat.lanes.push(Lane::new(note as u8, length));
    Ok(json!({"lane": pat.lanes.len() - 1}))
}

fn lane_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "pattern.lane_remove";
    let clip = clip_id(p, id)?;
    let i = index(p, "lane", id)?;
    let pat = pattern_mut(e, clip, id)?;
    if i >= pat.lanes.len() {
        return Err(bad(id, format!("lane {i} is out of range (the pattern has {})", pat.lanes.len())));
    }
    pat.lanes.remove(i);
    Ok(json!({"lanes": pat.lanes.len()}))
}

fn settings(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "pattern.settings";
    let clip = clip_id(p, id)?;
    let spb = match ranged(p, "steps_per_bar", 1, 32, id)? {
        Some(n) if !STEPS_PER_BAR.contains(&(n as u8)) => return Err(bad(id, "`steps_per_bar` must be 8, 12, 16, 24 or 32")),
        v => v,
    };
    let shuffle = ranged(p, "shuffle", 0, 100, id)?;
    let accent = ranged(p, "accent", 0, 127, id)?;
    let humanize = ranged(p, "humanize", 0, 100, id)?;
    let fill_every = ranged(p, "fill_every", 0, 16, id)?;
    let pat = pattern_mut(e, clip, id)?;
    if let Some(v) = spb {
        pat.steps_per_bar = v as u8;
    }
    if let Some(v) = shuffle {
        pat.shuffle = v as u8;
    }
    if let Some(v) = accent {
        pat.accent = v as u8;
    }
    if let Some(v) = humanize {
        pat.humanize = v as u8;
    }
    if let Some(v) = fill_every {
        pat.fill_every = v as u8;
    }
    let out = pat.clone();
    Ok(json!({"pattern": out}))
}

fn reroll(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "pattern.reroll";
    let pat = pattern_mut(e, clip_id(p, id)?, id)?;
    pat.reroll();
    Ok(json!({"seed": pat.seed}))
}

fn to_midi(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "pattern.to_midi";
    // The clip given, else the selected clips that are pattern clips.
    let clips: Vec<ClipId> = clip_ids_param(e, p).into_iter().filter(|c| pattern_clip(e, *c, id).is_ok()).collect();
    if clips.is_empty() {
        return Err(match p.get("clip") {
            Some(_) => pattern_clip(e, clip_id(p, id)?, id).err().unwrap_or_else(|| bad(id, "not a pattern clip")),
            None => bad(id, "no pattern clip selected"),
        });
    }
    let s = e.session_mut();
    for c in &clips {
        if let Some(clip) = s.find_clip_mut(*c) {
            clip.pattern = None;
        }
    }
    Ok(json!({"clips": clips.iter().map(|c| c.0).collect::<Vec<_>>()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use soundcraft_midi::PPQ;
    use soundcraft_model::ClipContent;

    const BAR: i64 = 4 * PPQ as i64;

    fn engine() -> (Engine, u64) {
        let mut e = Engine::default();
        assert!(e.execute("track.new", &json!({"count": 1, "kind": "instrument", "name": "Beat"})).is_ok());
        let t = e.session().tracks[0].id.0;
        (e, t)
    }

    fn notes(e: &Engine, clip: u64) -> Vec<(i64, u8, u8)> {
        match e.session().find_clip(ClipId(clip)).map(|(_, c)| &c.content) {
            Some(ClipContent::Midi { sequence }) => sequence.notes.iter().map(|n| (n.start, n.pitch, n.velocity)).collect(),
            _ => Vec::new(),
        }
    }

    fn new_clip(e: &mut Engine, t: u64, extra: Value) -> u64 {
        let mut p = json!({"track": t, "start": 0});
        if let (Some(o), Some(x)) = (p.as_object_mut(), extra.as_object()) {
            o.extend(x.clone());
        }
        match e.execute("pattern.new", &p) {
            Ok(v) => v["clip"].as_u64().unwrap_or(0),
            Err(err) => panic!("{err}"),
        }
    }

    #[test]
    fn a_new_pattern_is_an_empty_bar_on_the_grid() {
        let (mut e, t) = engine();
        let c = new_clip(&mut e, t, json!({}));
        let (_, clip) = e.session().find_clip(ClipId(c)).unwrap();
        let pc = clip.pattern.as_ref().unwrap();
        assert_eq!(pc.pattern.lanes.iter().map(|l| l.note).collect::<Vec<_>>(), DEFAULT_LANES.to_vec());
        assert_eq!(clip.start, 0);
        assert!(pc.seen.is_some(), "expanded after the command");
        assert!(notes(&e, c).is_empty());
    }

    #[test]
    fn steps_become_notes_and_undo_takes_them_back() {
        let (mut e, t) = engine();
        let c = new_clip(&mut e, t, json!({"bars": 2}));
        for s in [0, 4, 8, 12] {
            e.execute("pattern.step", &json!({"clip": c, "lane": 0, "step": s, "toggle": true})).unwrap();
        }
        let kicks: Vec<i64> = notes(&e, c).iter().map(|n| n.0).collect();
        assert_eq!(kicks, (0..8).map(|b| b * PPQ as i64).collect::<Vec<_>>(), "four steps loop over two bars");
        e.execute("pattern.step", &json!({"clip": c, "lane": 1, "step": 4, "on": true, "velocity": 64, "accent": true})).unwrap();
        let snare: Vec<_> = notes(&e, c).into_iter().filter(|n| n.1 == 38).collect();
        assert_eq!(snare.first().map(|n| n.2), Some(84), "accent adds the pattern's 20");
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(notes(&e, c).iter().all(|n| n.1 != 38));
        assert_eq!(notes(&e, c).len(), 8);
    }

    #[test]
    fn painting_a_run_and_clearing_it() {
        let (mut e, t) = engine();
        let c = new_clip(&mut e, t, json!({}));
        let v = e.execute("pattern.paint", &json!({"clip": c, "lane": 2, "from": 0, "to": 15, "on": true, "velocity": 64})).unwrap();
        assert_eq!(v["changed"], 16);
        assert_eq!(notes(&e, c).len(), 16);
        assert!(notes(&e, c).iter().all(|n| n.2 == 64));
        let undo_before = e.history_sessions().len();
        assert_eq!(e.execute("pattern.paint", &json!({"clip": c, "lane": 2, "from": 3, "to": 5, "on": true})).unwrap()["changed"], 0);
        assert_eq!(e.history_sessions().len(), undo_before, "painting what is already there is not a step");
        e.execute("pattern.paint", &json!({"clip": c, "lane": 2, "from": 15, "to": 8, "on": false})).unwrap();
        assert_eq!(notes(&e, c).len(), 8);
    }

    #[test]
    fn trimming_the_start_keeps_the_steps_in_place_and_moving_takes_them_along() {
        let (mut e, t) = engine();
        let c = new_clip(&mut e, t, json!({"bars": 2}));
        for s in [0, 4, 8, 12] {
            e.execute("pattern.step", &json!({"clip": c, "lane": 0, "step": s, "on": true})).unwrap();
        }
        let sr = e.session().sample_rate;
        let beat = e.session().tempo.tick_to_samples(PPQ as i64, sr);
        let abs = |e: &Engine| -> Vec<i64> {
            let (_, clip) = e.session().find_clip(ClipId(c)).unwrap();
            let t0 = e.session().tempo.samples_to_ticks(clip.start, sr);
            notes(e, c).iter().map(|n| t0 + n.0).collect()
        };
        let kicks = abs(&e);
        // Trim half a beat off the start: every kick still lands where it did.
        // Edits made outside a command (a UI gesture) are committed with `push_undo`, which refreshes.
        let snapshot = e.session_arc();
        if let Some(k) = e.session_mut().find_clip_mut(ClipId(c)) {
            k.start += beat / 2;
            k.length -= beat / 2;
        }
        e.push_undo("Trim", snapshot);
        let after = abs(&e);
        assert_eq!(after, kicks.iter().copied().filter(|&x| x >= PPQ as i64 / 2).collect::<Vec<_>>());
        // Move the clip a beat later: the kicks move with it.
        let snapshot = e.session_arc();
        if let Some(k) = e.session_mut().find_clip_mut(ClipId(c)) {
            k.start += beat;
        }
        e.push_undo("Move", snapshot);
        assert_eq!(abs(&e), after.iter().map(|x| x + PPQ as i64).collect::<Vec<_>>());
    }

    #[test]
    fn splitting_a_pattern_clip_keeps_every_hit_where_it_was() {
        let (mut e, t) = engine();
        let c = new_clip(&mut e, t, json!({"bars": 2}));
        e.execute("pattern.paint", &json!({"clip": c, "lane": 2, "from": 0, "to": 15, "on": true})).unwrap();
        e.execute("pattern.step", &json!({"clip": c, "lane": 0, "step": 3, "on": true})).unwrap();
        let sr = e.session().sample_rate;
        let absolute = |e: &Engine| -> Vec<(i64, u8)> {
            let mut v: Vec<(i64, u8)> = e.session().tracks[0]
                .clips()
                .iter()
                .flat_map(|clip| {
                    let t0 = e.session().tempo.samples_to_ticks(clip.start, sr);
                    let ClipContent::Midi { sequence } = &clip.content else { return Vec::new() };
                    sequence.notes.iter().map(|n| (t0 + n.start, n.pitch)).collect()
                })
                .collect();
            v.sort();
            v
        };
        let whole = absolute(&e);
        // Split off-grid (a third of the way into beat 3): both halves keep their pattern and phase.
        let at = e.session().tempo.tick_to_samples(2 * PPQ as i64 + PPQ as i64 / 3, sr);
        e.execute("edit.separate", &json!({"tracks": [t], "at": at})).unwrap();
        let clips = e.session().tracks[0].clips();
        assert_eq!(clips.len(), 2);
        assert!(clips.iter().all(|c| c.pattern.is_some()), "both halves stay pattern clips");
        assert_eq!(absolute(&e), whole);
    }

    #[test]
    fn editing_the_notes_by_hand_detaches_the_pattern() {
        let (mut e, t) = engine();
        let c = new_clip(&mut e, t, json!({}));
        e.execute("pattern.step", &json!({"clip": c, "lane": 0, "step": 0, "on": true})).unwrap();
        e.execute("midi.note_edit", &json!({"clip": c, "index": 0, "velocity": 30})).unwrap();
        let (_, clip) = e.session().find_clip(ClipId(c)).unwrap();
        assert!(clip.pattern.is_none(), "the hand edit wins");
        assert_eq!(notes(&e, c), vec![(0, 36, 30)]);
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().find_clip(ClipId(c)).unwrap().1.pattern.is_some(), "undo brings the pattern back");
    }

    #[test]
    fn lanes_settings_reroll_and_convert() {
        let (mut e, t) = engine();
        let c = new_clip(&mut e, t, json!({"lanes": [42]}));
        e.execute("pattern.paint", &json!({"clip": c, "lane": 0, "from": 0, "to": 15, "on": true})).unwrap();
        e.execute("pattern.lane", &json!({"clip": c, "lane": 0, "length": 3})).unwrap();
        assert_eq!(notes(&e, c).len(), 16, "a 3-step lane of hits still fills the bar");
        e.execute("pattern.lane", &json!({"clip": c, "lane": 0, "rate": "half"})).unwrap();
        assert_eq!(notes(&e, c).len(), 8);
        e.execute("pattern.lane_add", &json!({"clip": c, "note": 36})).unwrap();
        e.execute("pattern.step", &json!({"clip": c, "lane": 1, "step": 0, "on": true, "probability": 50})).unwrap();
        e.execute("pattern.settings", &json!({"clip": c, "shuffle": 50, "humanize": 10})).unwrap();
        let seed = pattern_clip(&e, ClipId(c), "t").unwrap().pattern.seed;
        let v = e.execute("pattern.reroll", &json!({"clip": c})).unwrap();
        assert_ne!(v["seed"].as_u64(), Some(seed));
        e.execute("pattern.step", &json!({"clip": c, "lane": 1, "step": 0, "condition": "1:2"})).unwrap();
        assert!(e.execute("pattern.step", &json!({"clip": c, "lane": 1, "step": 0, "condition": "3:2"})).is_err());
        let kept = notes(&e, c);
        e.execute("pattern.to_midi", &json!({"clip": c})).unwrap();
        assert!(e.session().find_clip(ClipId(c)).unwrap().1.pattern.is_none());
        assert_eq!(notes(&e, c), kept);
        assert!(e.execute("pattern.reroll", &json!({"clip": c})).is_err());
    }

    #[test]
    fn hostile_parameters_are_refused() {
        let (mut e, t) = engine();
        let c = new_clip(&mut e, t, json!({}));
        for p in [
            json!({"clip": c, "lane": 99, "step": 0, "on": true}),
            json!({"clip": c, "lane": 0, "step": 99, "on": true}),
            json!({"clip": c, "lane": 0, "step": 0, "velocity": 0}),
            json!({"clip": c, "lane": 0, "step": 0, "roll": 9}),
            json!({"clip": c, "lane": 0, "step": 0, "on": "yes"}),
            json!({"clip": 999_999, "lane": 0, "step": 0, "on": true}),
        ] {
            assert!(e.execute("pattern.step", &p).is_err(), "{p}");
        }
        assert!(e.execute("pattern.settings", &json!({"clip": c, "steps_per_bar": 7})).is_err());
        assert!(e.execute("pattern.lane", &json!({"clip": c, "lane": 0, "length": 0})).is_err());
        assert!(e.execute("pattern.new", &json!({"track": t, "bars": 0})).is_err());
        assert!(e.execute("pattern.new", &json!({"track": t, "lanes": [200]})).is_err());
        assert!(e.execute("track.new", &json!({"count": 1, "kind": "audio", "name": "Vox"})).is_ok());
        assert!(e.execute("pattern.new", &json!({"track": "Vox"})).is_err(), "not on audio tracks");
    }

    #[test]
    fn a_pattern_clip_survives_save_and_reopen() {
        let (mut e, t) = engine();
        let c = new_clip(&mut e, t, json!({}));
        e.execute("pattern.step", &json!({"clip": c, "lane": 0, "step": 0, "on": true})).unwrap();
        let json = serde_json::to_string(e.session()).unwrap();
        let mut back: soundcraft_model::Session = serde_json::from_str(&json).unwrap();
        crate::pattern::refresh_in(&mut back);
        let (_, clip) = back.find_clip(ClipId(c)).unwrap();
        assert!(clip.pattern.is_some(), "the fingerprint still matches after a round trip");
        assert_eq!(BAR % 16, 0);
    }
}
