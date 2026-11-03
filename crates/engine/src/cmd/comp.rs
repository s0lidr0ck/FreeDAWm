//! Comping with take layers (Studio One style).
//!
//! Each take layer is a playlist of the track; the comp is the active playlist. Comp segments
//! remember the layer they came from ([`soundcraft_model::Clip::from_layer`]). Swiping a range of
//! a layer copies it into the comp; the joins get short crossfades.

use super::*;
use crate::cmd;
use crate::edit;
use serde_json::json;
use soundcraft_model::{Clip, FadeShape, Playlist, Session, Track};

/// The crossfade at each join a swipe makes, in seconds.
pub const JOIN_FADE_SECONDS: f64 = 0.010;

/// Refusal of a layer delete while the comp still plays part of it.
pub const LAYER_IN_COMP: &str = "Remove it from the comp first";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "comp.swipe",
            "Comp",
            [],
            None,
            "{track, layer: playlist index, start, end, add?: false} (that range of the layer replaces the comp there; `add`: only where the comp is empty)",
            has_tracks,
            swipe
        ),
        cmd!(
            "comp.cycle",
            "Comp",
            [],
            None,
            "{track, start?, end? (else the edit selection), dir: up|down} (that comp range switches to the layer above/below with audio there)",
            has_tracks,
            cycle
        ),
        cmd!("comp.layer_new", "New Layer", [], None, "{track?, name?} (an empty take layer, the record target)", has_tracks, layer_new),
        cmd!(
            "comp.layer_delete",
            "Delete Take",
            [],
            None,
            "{track, layer, remove?: false} (refused while the comp uses it, unless `remove` takes it out of the comp first)",
            has_tracks,
            layer_delete
        ),
        cmd!(
            "comp.layer_record_target",
            "Record Into This Layer",
            [],
            None,
            "{track, layer | null} (the next recording pass goes into that layer instead of a new one)",
            has_tracks,
            record_target
        ),
        cmd!("comp.layer_rename", "Rename Layer", [], None, "{track, layer, name}", has_tracks, layer_rename),
        cmd!(
            "comp.layer_make_comp",
            "Make Comp from This Take",
            [],
            None,
            "{track, layer} (the whole comp becomes that take)",
            has_tracks,
            make_comp
        ),
        cmd!(noundo "comp.layers", "Show Layers", [], None, "{track?, open?: toggle}", has_tracks, show_layers),
    ]
}

fn track_of(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    let t = track_param(e, id, p, "track")?.or_else(|| e.target_tracks().first().copied()).ok_or_else(|| bad(id, "`track` required"))?;
    if !e.session().track(t).is_some_and(|tr| tr.kind.has_playlist()) {
        return Err(bad(id, "that track has no takes"));
    }
    Ok(t)
}

fn layer_of(e: &Engine, id: &str, p: &Value, t: TrackId) -> Result<usize> {
    let l = p.get("layer").and_then(Value::as_u64).ok_or_else(|| bad(id, "`layer` (playlist index) required"))?;
    let l = usize::try_from(l).map_err(|_| bad(id, "no such layer"))?;
    if e.session().track(t).is_some_and(|tr| tr.is_layer(l)) { Ok(l) } else { Err(bad(id, "no such layer (the comp itself is not a layer)")) }
}

fn layer_name(tr: &Track, layer: usize) -> String {
    tr.playlists.get(layer).map_or_else(|| "That take".into(), |p| p.name.clone())
}

/// Whether layer `layer` has audio (a clip) in `r`.
pub fn layer_has_audio(tr: &Track, layer: usize, r: Range) -> bool {
    tr.playlists.get(layer).is_some_and(|p| p.clips_in(r).next().is_some())
}

/// The ranges of `r` the comp leaves empty.
fn comp_gaps(tr: &Track, r: Range) -> Vec<Range> {
    let mut gaps = Vec::new();
    let mut at = r.start;
    for c in tr.clips().iter().filter(|c| c.range().overlaps(&r)) {
        if c.start > at {
            gaps.push(Range::new(at, c.start.min(r.end)));
        }
        at = at.max(c.end());
    }
    if at < r.end {
        gaps.push(Range::new(at, r.end));
    }
    gaps.retain(|g| !g.is_empty());
    gaps
}

/// The material of layer `layer` inside `r`, relative to `r.start` (as [`edit::copy_range`]).
fn copy_layer(s: &mut Session, t: TrackId, layer: usize, r: Range) -> Vec<Clip> {
    let Some(active) = s.track(t).map(|tr| tr.active_playlist) else { return Vec::new() };
    if let Some(tr) = s.track_mut(t) {
        tr.active_playlist = layer;
    }
    let clips = edit::copy_range(s, t, r);
    if let Some(tr) = s.track_mut(t) {
        tr.active_playlist = active;
    }
    clips
}

/// Put layer `layer`'s material in `r` on the comp in place of what's there (silence where the
/// layer has none), marked as from that layer. Returns the new comp clips.
fn put(s: &mut Session, t: TrackId, layer: usize, r: Range) -> Vec<ClipId> {
    let clips = copy_layer(s, t, layer, r);
    edit::clear_range(s, t, r, false);
    let ids = edit::paste(s, t, r.start, &clips, r.len(), false).unwrap_or_default();
    if let Some(pl) = s.track_mut(t).and_then(Track::playlist_mut) {
        for c in pl.clips.iter_mut().filter(|c| ids.contains(&c.id)) {
            c.from_layer = Some(layer);
        }
    }
    ids
}

/// The join at `x` on the comp: two pieces of one take that continue each other are healed into
/// one clip; two different pieces get a short crossfade.
fn join_at(s: &mut Session, t: TrackId, x: Samples) {
    let Some(tr) = s.track(t) else { return };
    let (Some(a), Some(b)) = (tr.clips().iter().find(|c| c.end() == x), tr.clips().iter().find(|c| c.start == x)) else { return };
    let continues = a.from_layer == b.from_layer
        && matches!((&a.content, &b.content), (soundcraft_model::ClipContent::Audio { source: sa, offset: oa }, soundcraft_model::ClipContent::Audio { source: sb, offset: ob }) if sa == sb && oa.saturating_add(a.length) == *ob);
    if continues {
        edit::heal(s, t, Range::point(x));
        return;
    }
    let half = (s.sample_rate.samples(JOIN_FADE_SECONDS) / 2).max(1);
    edit::create_fades(s, t, Range::new(x.saturating_sub(half), x.saturating_add(half)), FadeShape::EqualPower);
}

/// Swipe `r` of `layer` into the comp (`add`: only into the comp's empty parts). Errors when the
/// layer has no audio there.
pub fn swipe_range(s: &mut Session, t: TrackId, layer: usize, r: Range, add: bool) -> std::result::Result<Vec<ClipId>, String> {
    let Some(tr) = s.track(t) else { return Err("no track".into()) };
    if r.is_empty() {
        return Err("give a range (`start` < `end`)".into());
    }
    if !layer_has_audio(tr, layer, r) {
        return Err(format!("{} has no audio there", layer_name(tr, layer)));
    }
    let parts: Vec<Range> = if add { comp_gaps(tr, r).into_iter().filter(|g| layer_has_audio(tr, layer, *g)).collect() } else { vec![r] };
    if parts.is_empty() {
        return Err("the comp is not empty there".into());
    }
    let mut ids = Vec::new();
    for part in &parts {
        ids.extend(put(s, t, layer, *part));
    }
    for part in &parts {
        join_at(s, t, part.start);
        join_at(s, t, part.end);
    }
    Ok(ids)
}

fn swipe(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_of(e, "comp.swipe", p)?;
    let layer = layer_of(e, "comp.swipe", p, t)?;
    let r = range_param(e, "comp.swipe", p)?;
    let add = bool_or(p, "add", false);
    let ids = swipe_range(e.session_mut(), t, layer, r, add).map_err(|m| bad("comp.swipe", m))?;
    Ok(json!({"clips": ids, "layer": layer}))
}

/// The layer `dir` (`down` = later in the list) of the one the comp plays at `r`, among the layers
/// with audio in `r`.
pub fn cycle_target(tr: &Track, r: Range, down: bool) -> Option<usize> {
    let order: Vec<usize> = tr.layers().map(|(i, _)| i).collect();
    // The comp segment covering most of `r` (its crossfaded neighbours reach in a little).
    let current =
        tr.clips().iter().filter_map(|c| c.range().intersect(&r).map(|i| (i.len(), c))).max_by_key(|(n, _)| *n).and_then(|(_, c)| c.from_layer);
    let pos = current.and_then(|c| order.iter().position(|&i| i == c));
    let with_audio = |i: &&usize| layer_has_audio(tr, **i, r);
    match (pos, down) {
        (Some(p), true) => order.iter().skip(p + 1).find(with_audio).copied(),
        (Some(p), false) => order.iter().take(p).rev().find(with_audio).copied(),
        (None, true) => order.iter().find(with_audio).copied(),
        (None, false) => order.iter().rev().find(with_audio).copied(),
    }
}

fn cycle(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_of(e, "comp.cycle", p)?;
    let r = range_param(e, "comp.cycle", p)?;
    if r.is_empty() {
        return Err(bad("comp.cycle", "select a comp segment (or give `start` and `end`)"));
    }
    let down = match str_param(p, "dir") {
        Some("down") => true,
        Some("up") => false,
        _ => return Err(bad("comp.cycle", "`dir` must be up or down")),
    };
    let tr = e.session().track(t).ok_or_else(|| bad("comp.cycle", "no track"))?;
    let layer =
        cycle_target(tr, r, down).ok_or_else(|| bad("comp.cycle", format!("no take {} has audio there", if down { "below" } else { "above" })))?;
    let ids = swipe_range(e.session_mut(), t, layer, r, false).map_err(|m| bad("comp.cycle", m))?;
    Ok(json!({"clips": ids, "layer": layer}))
}

/// The next "Take N" number of a track: one past the highest so far, and past the layer count.
fn next_take_number(tr: &Track) -> usize {
    let named = tr.layers().filter_map(|(_, p)| p.name.strip_prefix("Take ").and_then(|n| n.trim().parse::<usize>().ok())).max().unwrap_or(0);
    named.max(tr.layers().count()).saturating_add(1)
}

/// Add an empty take layer to track `t` (shown, the record target). Returns its index.
pub fn add_layer(s: &mut Session, t: TrackId, name: Option<String>) -> Option<usize> {
    first_take(s, t);
    let tr = s.track_mut(t)?;
    let n = next_take_number(tr);
    let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Take {n}"));
    tr.playlists.push(Playlist { name, clips: Vec::new(), take_color: Some(((n.saturating_sub(1)) % 8) as u8) });
    let i = tr.playlists.len() - 1;
    tr.layers_open = true;
    tr.record_layer = Some(i);
    Some(i)
}

/// A track starting to comp with audio on it and no takes yet: that audio becomes "Take 1", the
/// comp playing it.
fn first_take(s: &mut Session, t: TrackId) {
    let Some(tr) = s.track(t) else { return };
    if tr.layers().any(|(_, p)| !p.clips.is_empty()) || tr.clips().is_empty() {
        return;
    }
    let mut clips = tr.clips().to_vec();
    for c in &mut clips {
        c.id = s.new_clip_id();
        c.from_layer = None;
    }
    let Some(tr) = s.track_mut(t) else { return };
    let n = next_take_number(tr);
    tr.playlists.push(Playlist { name: format!("Take {n}"), clips, take_color: Some(((n.saturating_sub(1)) % 8) as u8) });
    let l = tr.playlists.len() - 1;
    if let Some(pl) = tr.playlist_mut() {
        for c in &mut pl.clips {
            c.from_layer = Some(l);
        }
    }
}

/// Whether recording on the track makes take layers: its layers are open, it has a record target
/// or take layers. Other tracks record as playlists (a new playlist per loop pass).
pub fn comping(tr: &Track) -> bool {
    tr.kind == soundcraft_model::TrackKind::Audio
        && (tr.layers_open || tr.record_layer.is_some() || tr.playlists.iter().any(|p| p.take_color.is_some()))
}

/// Place a recorded pass on a comping track: into its record target (used once) or a new take
/// layer, and into the comp marked as from that layer. Returns the comp clip's id.
pub fn record_pass(s: &mut Session, t: TrackId, clip: Clip) -> ClipId {
    let target = s.track_mut(t).and_then(|tr| tr.record_layer.take().filter(|l| tr.is_layer(*l)));
    let layer = target.or_else(|| {
        let l = add_layer(s, t, None);
        if let Some(tr) = s.track_mut(t) {
            tr.record_layer = None;
        }
        l
    });
    let Some(layer) = layer else {
        let id = clip.id;
        edit::place_clip(s, t, clip);
        return id;
    };
    let Some(active) = s.track(t).map(|tr| tr.active_playlist) else { return clip.id };
    if let Some(tr) = s.track_mut(t) {
        tr.active_playlist = layer;
    }
    edit::place_clip(s, t, clip.clone());
    if let Some(tr) = s.track_mut(t) {
        tr.active_playlist = active;
        tr.layers_open = true;
    }
    let mut c = clip;
    c.id = s.new_clip_id();
    c.from_layer = Some(layer);
    let id = c.id;
    edit::place_clip(s, t, c);
    id
}

fn layer_new(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_of(e, "comp.layer_new", p)?;
    let name = str_param(p, "name").map(str::to_string);
    let i = add_layer(e.session_mut(), t, name).ok_or_else(|| bad("comp.layer_new", "no track"))?;
    Ok(json!({"layer": i}))
}

fn layer_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_of(e, "comp.layer_delete", p)?;
    let layer = layer_of(e, "comp.layer_delete", p, t)?;
    let remove = bool_or(p, "remove", false);
    let s = e.session_mut();
    let Some(tr) = s.track_mut(t) else { return Err(bad("comp.layer_delete", "no track")) };
    let used = tr.clips().iter().any(|c| c.from_layer == Some(layer));
    if used && !remove {
        return Err(EngineError::Disabled("comp.layer_delete".into(), LAYER_IN_COMP.into()));
    }
    if let Some(pl) = tr.playlist_mut() {
        pl.clips.retain(|c| c.from_layer != Some(layer));
    }
    tr.playlists.remove(layer);
    if tr.active_playlist > layer {
        tr.active_playlist -= 1;
    }
    let shift = |l: Option<usize>| match l {
        Some(i) if i == layer => None,
        Some(i) if i > layer => Some(i - 1),
        other => other,
    };
    tr.record_layer = shift(tr.record_layer);
    for pl in &mut tr.playlists {
        for c in &mut pl.clips {
            c.from_layer = shift(c.from_layer);
        }
    }
    Ok(json!({"removed_from_comp": used}))
}

fn record_target(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_of(e, "comp.layer_record_target", p)?;
    let layer = if p.get("layer").is_some_and(Value::is_null) { None } else { Some(layer_of(e, "comp.layer_record_target", p, t)?) };
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.record_layer = layer;
    }
    Ok(json!({"layer": layer}))
}

fn layer_rename(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_of(e, "comp.layer_rename", p)?;
    let layer = layer_of(e, "comp.layer_rename", p, t)?;
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("comp.layer_rename", "`name` required"))?.to_string();
    if let Some(pl) = e.session_mut().track_mut(t).and_then(|tr| tr.playlists.get_mut(layer)) {
        pl.name = name;
    }
    Ok(json!({}))
}

fn make_comp(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_of(e, "comp.layer_make_comp", p)?;
    let layer = layer_of(e, "comp.layer_make_comp", p, t)?;
    let s = e.session_mut();
    let Some(src) = s.track(t).and_then(|tr| tr.playlists.get(layer)).cloned() else { return Err(bad("comp.layer_make_comp", "no such layer")) };
    let mut clips = src.clips;
    for c in &mut clips {
        c.id = s.new_clip_id();
        c.from_layer = Some(layer);
    }
    let ids: Vec<ClipId> = clips.iter().map(|c| c.id).collect();
    if let Some(pl) = s.track_mut(t).and_then(Track::playlist_mut) {
        pl.clips = clips;
        pl.sort();
    }
    Ok(json!({"clips": ids}))
}

fn show_layers(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_of(e, "comp.layers", p)?;
    let Some(tr) = e.session_mut().track_mut(t) else { return Err(bad("comp.layers", "no track")) };
    tr.layers_open = p.get("open").and_then(Value::as_bool).unwrap_or(!tr.layers_open);
    Ok(json!({"open": tr.layers_open}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{Source, SourceId};

    const LEN: Samples = 480_000;

    /// A track "Vox" whose comp (playlist 0) is empty and whose layers 1-4 each hold one take
    /// over 0..LEN (with handles), from sources 501-504.
    fn four_takes() -> (Engine, TrackId) {
        let mut e = Engine::default();
        let t = TrackId(e.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap());
        let s = e.session_mut();
        for k in 1..=4u64 {
            s.sources.push(Source {
                id: SourceId(500 + k),
                name: format!("take{k}"),
                path: format!("take{k}.wav"),
                channels: 1,
                frames: 1_000_000,
                sample_rate: 48_000,
                format: soundcraft_audio_io::FileFormat::Wav,
                time_reference: 0,
                unsaved: false,
            });
            let id = s.new_clip_id();
            let n = add_layer(s, t, None).unwrap();
            let tr = s.track_mut(t).unwrap();
            tr.playlists[n].clips.push(Clip::audio(id, format!("take{k}"), SourceId(500 + k), 10_000, 0, LEN));
        }
        (e, t)
    }

    fn comp(e: &Engine, t: TrackId) -> Vec<(Samples, Samples, Option<usize>)> {
        e.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.end(), c.from_layer)).collect()
    }

    #[test]
    fn swipe_replaces_only_the_range_and_records_its_layer() {
        let (mut e, t) = four_takes();
        let undo_before = e.undo_history().len();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 1, "start": 0, "end": LEN})).unwrap();
        assert_eq!(comp(&e, t), [(0, LEN, Some(1))]);
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 3, "start": 100_000, "end": 200_000})).unwrap();
        let c = comp(&e, t);
        assert_eq!(c.iter().map(|x| x.2).collect::<Vec<_>>(), [Some(1), Some(3), Some(1)]);
        // The joins are crossfaded: each side reaches 5 ms into the other.
        let half = 240;
        assert_eq!((c[0].1, c[1].0, c[1].1, c[2].0), (100_000 + half, 100_000 - half, 200_000 + half, 200_000 - half));
        let clips = e.session().track(t).unwrap().clips().to_vec();
        assert_eq!((clips[0].fade_out.len, clips[1].fade_in.len, clips[1].fade_out.len, clips[2].fade_in.len), (480, 480, 480, 480));
        // The comp's take 3 piece plays take 3's audio from the same place.
        assert_eq!(clips[1].source_offset(), 10_000 + 100_000 - half);
        // One undo step each, named Comp.
        assert_eq!(e.undo_history().len(), undo_before + 2);
        assert_eq!(e.undo_label(), Some("Comp"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(comp(&e, t), [(0, LEN, Some(1))]);
    }

    #[test]
    fn swiping_a_take_back_heals_it_and_empty_layers_refuse() {
        let (mut e, t) = four_takes();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 1, "start": 0, "end": LEN})).unwrap();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 2, "start": 100_000, "end": 200_000})).unwrap();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 1, "start": 50_000, "end": 300_000})).unwrap();
        // Take 1 again over the whole take-2 piece: one take-1 clip again (no needless crossfades).
        assert_eq!(comp(&e, t).iter().map(|x| x.2).collect::<Vec<_>>(), [Some(1)]);
        let n = add_layer(e.session_mut(), t, None).unwrap();
        assert!(e.execute("comp.swipe", &json!({"track": t.0, "layer": n, "start": 0, "end": 10})).is_err());
        // The comp is not a layer.
        assert!(e.execute("comp.swipe", &json!({"track": t.0, "layer": 0, "start": 0, "end": 10})).is_err());
    }

    #[test]
    fn add_mode_fills_only_the_empty_parts() {
        let (mut e, t) = four_takes();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 1, "start": 0, "end": 100_000})).unwrap();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 2, "start": 300_000, "end": 400_000})).unwrap();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 4, "start": 50_000, "end": 350_000, "add": true})).unwrap();
        let c = comp(&e, t);
        assert_eq!(c.iter().map(|x| x.2).collect::<Vec<_>>(), [Some(1), Some(4), Some(2)]);
        // The neighbours keep their ground (up to the crossfade overlap).
        assert!(c[0].0 == 0 && c[0].1 >= 100_000 && c[2].0 <= 300_000 && c[2].1 == 400_000);
        assert!(c[1].0 < 100_000 && c[1].1 > 300_000);
        // Nothing empty left there: refused.
        assert!(e.execute("comp.swipe", &json!({"track": t.0, "layer": 3, "start": 50_000, "end": 350_000, "add": true})).is_err());
    }

    #[test]
    fn cycle_walks_the_layers_with_audio_there() {
        let (mut e, t) = four_takes();
        // Layer 3 has nothing at 100k-200k.
        e.session_mut().track_mut(t).unwrap().playlists[3].clips[0].start = 300_000;
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 1, "start": 0, "end": LEN})).unwrap();
        let r = json!({"track": t.0, "start": 100_000, "end": 200_000});
        let go = |e: &mut Engine, dir: &str| {
            let mut p = r.clone();
            p["dir"] = json!(dir);
            e.execute("comp.cycle", &p).map(|v| v["layer"].as_u64().unwrap())
        };
        assert_eq!(go(&mut e, "down").unwrap(), 2);
        assert_eq!(go(&mut e, "down").unwrap(), 4, "layer 3 has no audio there");
        assert!(go(&mut e, "down").is_err(), "no layer below the last");
        assert_eq!(go(&mut e, "up").unwrap(), 2);
        assert_eq!(go(&mut e, "up").unwrap(), 1);
        assert!(go(&mut e, "up").is_err());
        assert!(e.execute("comp.cycle", &json!({"track": t.0, "start": 0, "end": 10, "dir": "sideways"})).is_err());
    }

    #[test]
    fn new_blank_layer_is_named_coloured_and_the_record_target() {
        let (mut e, t) = four_takes();
        let v = e.execute("comp.layer_new", &json!({"track": t.0})).unwrap();
        let tr = e.session().track(t).unwrap();
        let n = v["layer"].as_u64().unwrap() as usize;
        let pl = &tr.playlists[n];
        assert_eq!((pl.name.as_str(), pl.clips.len(), pl.take_color), ("Take 5", 0, Some(4)));
        assert_eq!((tr.record_layer, tr.layers_open), (Some(n), true));
        e.execute("comp.layer_record_target", &json!({"track": t.0, "layer": 2})).unwrap();
        assert_eq!(e.session().track(t).unwrap().record_layer, Some(2));
        e.execute("comp.layer_record_target", &json!({"track": t.0, "layer": null})).unwrap();
        assert_eq!(e.session().track(t).unwrap().record_layer, None);
        e.execute("comp.layer_rename", &json!({"track": t.0, "layer": 2, "name": "Best"})).unwrap();
        assert_eq!(e.session().track(t).unwrap().playlists[2].name, "Best");
        e.execute("comp.layers", &json!({"track": t.0})).unwrap();
        assert!(!e.session().track(t).unwrap().layers_open);
    }

    #[test]
    fn delete_is_refused_while_the_comp_uses_the_take() {
        let (mut e, t) = four_takes();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 2, "start": 0, "end": 100_000})).unwrap();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 4, "start": 100_000, "end": 200_000})).unwrap();
        let err = e.execute("comp.layer_delete", &json!({"track": t.0, "layer": 2})).unwrap_err();
        assert!(err.to_string().contains(LAYER_IN_COMP), "{err}");
        // An unused one goes; later layers move up and the comp follows.
        e.execute("comp.layer_delete", &json!({"track": t.0, "layer": 3})).unwrap();
        assert_eq!(comp(&e, t).iter().map(|x| x.2).collect::<Vec<_>>(), [Some(2), Some(3)]);
        assert_eq!(e.session().track(t).unwrap().playlists[3].name, "Take 4");
        // Remove and Delete: its pieces leave the comp too.
        e.execute("comp.layer_delete", &json!({"track": t.0, "layer": 2, "remove": true})).unwrap();
        assert_eq!(comp(&e, t).iter().map(|x| x.2).collect::<Vec<_>>(), [Some(2)]);
        assert_eq!(e.session().track(t).unwrap().playlists.len(), 3);
    }

    #[test]
    fn make_comp_from_a_take() {
        let (mut e, t) = four_takes();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": 2, "start": 0, "end": 100_000})).unwrap();
        e.execute("comp.layer_make_comp", &json!({"track": t.0, "layer": 3})).unwrap();
        assert_eq!(comp(&e, t), [(0, LEN, Some(3))]);
    }
}
