//! Track menu commands.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ChannelFormat, Group, GroupId, ModuleRef, Playlist, Route, Session, Track, TrackHeight, TrackId, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.new",
            "New...",
            ["Track"],
            Some("Cmd+Shift+N"),
            "{count?: 1, format?: mono|stereo|5.1…, kind?: audio|aux|master|midi|instrument|vca|folder, name?, timebase?: samples|ticks, instrument?: plugin id}",
            always,
            new_track
        ),
        cmd!("track.group", "Group...", ["Track"], Some("Cmd+G"), "{name?, tracks?, edit?: true, mix?: true}", has_selection, group),
        cmd!("track.ungroup", "Delete Group", [], None, "{group: id|name}", always, delete_group),
        cmd!("track.group_toggle", "Toggle Group Active", [], None, "{group: id|name}", always, toggle_group),
        cmd!("track.group_modify", "Modify Group", [], None, "{group: id|name, name?, tracks?, edit?, mix?}", always, modify_group),
        cmd!(
            "track.duplicate",
            "Duplicate...",
            ["Track"],
            Some("Alt+Shift+D"),
            "{tracks?, playlists?: true, automation?: true, inserts?: true, sends?: true}",
            has_selection,
            duplicate
        ),
        cmd!("track.split_into_mono", "Split into Mono", ["Track"], None, "{tracks?}", has_selection, split_mono),
        cmd!("track.make_inactive", "Make Inactive", ["Track"], None, "{tracks?, inactive?: bool}", has_selection, |e, p| flag(
            e,
            p,
            "track.make_inactive",
            |t, v| t.inactive = v,
            |t| t.inactive
        )),
        cmd!(
            "track.move_to_new_folder",
            "Move to New Folder...",
            ["Track"],
            Some("Cmd+Alt+Shift+N"),
            "{tracks?, name?}",
            has_selection,
            move_to_folder
        ),
        cmd!("track.move_to_folder", "Move to Folder", [], None, "{tracks?, folder}", has_selection, move_into_folder),
        cmd!("track.remove_from_folder", "Remove from Folder", [], None, "{tracks?}", has_selection, remove_from_folder),
        cmd!(
            "track.change_width",
            "Change Track Width",
            ["Track", "Change Track Width"],
            None,
            "{tracks?, format: mono|stereo|…}",
            has_selection,
            change_width
        ),
        cmd!("track.delete", "Delete", ["Track"], None, "{tracks?}", has_selection, delete),
        cmd!("track.rename", "Rename Track", [], None, "{track, name}", has_tracks, rename),
        cmd!("track.comments", "Track Comments", [], None, "{track, comments}", has_tracks, comments),
        cmd!("track.color", "Track Color", [], None, "{tracks?, color: [r,g,b] | palette index}", has_selection, color),
        cmd!(
            "track.height",
            "Track Height",
            [],
            None,
            "{tracks?, height: micro|mini|small|medium|large|jumbo|extreme | points}",
            has_selection,
            height
        ),
        cmd!("track.height_up", "Increase Track Height", [], Some("Ctrl+Up"), "{tracks?} — one preset taller", has_selection, |e, p| height_step(
            e, p, true
        )),
        cmd!("track.height_down", "Decrease Track Height", [], Some("Ctrl+Down"), "{tracks?} — one preset shorter", has_selection, |e, p| {
            height_step(e, p, false)
        }),
        cmd!(
            "track.view",
            "Track View",
            [],
            None,
            "{tracks?, view: waveform|pitch|blocks|notes|volume|pan|mute|<automation id>}",
            has_selection,
            view
        ),
        cmd!("track.hide", "Hide Track", [], None, "{tracks?, hidden?: bool}", has_selection, |e, p| flag(
            e,
            p,
            "track.hide",
            |t, v| t.hidden = v,
            |t| t.hidden
        )),
        cmd!(
            "track.auto_lane",
            "Show Automation Lane",
            [],
            None,
            "{tracks?, param: automation id, show?: bool (absent toggles), after?: automation id (shows it right under that lane)} → {shown}",
            has_selection,
            auto_lane
        ),
        cmd!(
            "track.auto_lanes",
            "Show Automation Lanes",
            [],
            None,
            "{tracks?, show: data|none} — data shows a lane for every parameter with automation (volume when none has any), none hides them all",
            has_selection,
            auto_lanes
        ),
        cmd!("track.pin", "Pin Track", [], None, "{tracks?}", has_selection, |e, p| flag(e, p, "track.pin", |t, v| t.pinned = v, |t| t.pinned)),
        cmd!(
            "track.move",
            "Move Track",
            [],
            None,
            "{track, to: index, count?: n (the track and the n-1 after it move as one block, e.g. a folder with its members; `to` counts without the block)}",
            has_tracks,
            move_track
        ),
        cmd!(
            "track.freeze",
            "Freeze",
            ["Track"],
            None,
            "{tracks?, freeze?: bool, upto?: module id (freeze down to and including it; the modules after stay live)}",
            has_selection,
            super::freeze::freeze
        ),
        cmd!("track.commit", "Commit...", ["Track"], Some("Alt+Shift+C"), "{tracks?}", has_selection, commit),
        cmd!("track.bounce", "Bounce...", ["Track"], Some("Cmd+Alt+Shift+B"), "{tracks?}", has_selection, commit),
        cmd!("track.bypass_inserts", "All", ["Track", "Bypass Inserts"], None, "{tracks?, bypass?: bool}", has_selection, bypass_inserts),
        cmd!("track.mute_sends", "All", ["Track", "Mute Sends"], None, "{tracks?, mute?: bool}", has_selection, mute_sends),
        cmd!("track.set_record_to_input_only", "Set Record Tracks to Input Only", ["Track"], Some("Alt+K"), "{}", always, |e, _| {
            let s = e.session_mut();
            let mut n = 0;
            for t in &mut s.tracks {
                if t.mixer.record_arm {
                    t.mixer.input_monitor = soundcraft_model::InputMonitor::On;
                    n += 1;
                }
            }
            Ok(json!({"tracks": n}))
        }),
        cmd!(noundo "track.scroll_to", "Scroll to Track...", ["Track"], Some("Cmd+Alt+F"), "{track}", has_tracks, |e, p| {
            let t = track_param(e, "track.scroll_to", p, "track")?.ok_or_else(|| bad("track.scroll_to", "`track` required"))?;
            let s = e.session_mut();
            s.edit.selected_tracks = vec![t];
            Ok(json!({"track": t}))
        }),
        cmd!("track.clear_clip_indicators", "Clear All Clip Indicators", ["Track"], Some("Alt+C"), "{}", always, |_, _| Ok(json!({"cleared": true}))),
        cmd!("track.clear_trim_automation", "Clear Trim Automation", ["Track"], None, "{tracks?}", has_selection, super::track_more::clear_trim),
        cmd!("track.create_click", "Create Click Track", ["Track"], None, "{}", always, create_click),
        cmd!("track.playlist_new", "New Playlist", [], None, "{track?, name?}", has_selection, |e, p| new_playlist(e, p, false)),
        cmd!("track.playlist_duplicate", "Duplicate Playlist", [], None, "{track?, name?}", has_selection, |e, p| new_playlist(e, p, true)),
        cmd!("track.playlist_select", "Select Playlist", [], None, "{track?, index | name}", has_selection, select_playlist),
        cmd!("track.playlist_delete_unused", "Delete Unused Playlists", [], None, "{track?}", has_selection, |e, p| {
            let tracks = tracks_required(e, "track.playlist_delete_unused", p)?;
            let s = e.session_mut();
            let mut n = 0;
            for t in tracks {
                if let Some(tr) = s.track_mut(t)
                    && let Some(active) = tr.playlists.get(tr.active_playlist).cloned()
                {
                    n += tr.playlists.len() - 1;
                    tr.playlists = vec![active];
                    tr.active_playlist = 0;
                }
            }
            Ok(json!({"deleted": n}))
        }),
        cmd!(noundo "track.folder_toggle", "Open/Close Folder", [], None, "{track, open?: bool}", has_tracks, |e, p| {
            let t = track_param(e, "track.folder_toggle", p, "track")?.ok_or_else(|| bad("track.folder_toggle", "`track` required"))?;
            let v = p.get("open").and_then(Value::as_bool);
            let s = e.session_mut();
            let tr = s.track_mut(t).ok_or_else(|| bad("track.folder_toggle", "no track"))?;
            tr.folder_open = v.unwrap_or(!tr.folder_open);
            Ok(json!({"open": tr.folder_open}))
        }),
        cmd!("track.vca", "Assign to VCA", [], None, "{tracks?, vca: VCA track id|name | null}", has_selection, |e, p| {
            let tracks = tracks_required(e, "track.vca", p)?;
            let vca = match p.get("vca") {
                Some(Value::Null) => None,
                _ => track_param(e, "track.vca", p, "vca")?,
            };
            if let Some(v) = vca
                && e.session().track(v).is_none_or(|t| t.kind != TrackKind::Vca)
            {
                return Err(bad("track.vca", "`vca` must be a VCA Master track"));
            }
            let s = e.session_mut();
            for t in &tracks {
                if let Some(tr) = s.track_mut(*t) {
                    tr.mixer.vca = vca.map(|v| v.0);
                }
            }
            Ok(json!({"vca": vca}))
        }),
        cmd!(
            "track.playlist_promote",
            "Promote to Main Playlist",
            [],
            None,
            "{track, playlist: index, start?, end?} (copy that range from an alternate playlist into the active one)",
            has_tracks,
            |e, p| {
                let t = track_param(e, "track.playlist_promote", p, "track")?.ok_or_else(|| bad("track.playlist_promote", "`track` required"))?;
                let idx =
                    p.get("playlist").and_then(Value::as_u64).ok_or_else(|| bad("track.playlist_promote", "`playlist` index required"))? as usize;
                let r = range_param(e, "track.playlist_promote", p)?;
                if r.is_empty() {
                    return Err(bad("track.playlist_promote", "give `start` and `end`"));
                }
                let s = e.session_mut();
                let (src, active) = {
                    let tr = s.track(t).ok_or_else(|| bad("track.playlist_promote", "no track"))?;
                    (tr.playlists.get(idx).cloned().ok_or_else(|| bad("track.playlist_promote", "no such playlist"))?, tr.active_playlist)
                };
                if idx == active {
                    return Err(bad("track.playlist_promote", "that playlist is already active"));
                }
                // Copy the clips of the source playlist inside the range (via a temporary active switch).
                if let Some(tr) = s.track_mut(t) {
                    tr.active_playlist = idx;
                }
                let clips = crate::edit::copy_range(s, t, r);
                if let Some(tr) = s.track_mut(t) {
                    tr.active_playlist = active;
                }
                let _ = src;
                let ids = crate::edit::paste(s, t, r.start, &clips, r.len(), false).unwrap_or_default();
                Ok(json!({"clips": ids}))
            }
        ),
        cmd!(
            "track.input",
            "Track Input",
            [],
            None,
            "{tracks?, input: none|bus name|hardware name|musician:N (1-6)|module:<track>/<module index or name>|[module:…, module:…]|midi:all|midi:<port name>, channel?: 0 (Omni)|1-16}",
            has_selection,
            |e, p| route(e, p, true)
        ),
        cmd!("track.output", "Track Output", [], None, "{tracks?, output: main|none|bus name}", has_selection, |e, p| route(e, p, false)),
        cmd!("track.timebase", "Track Timebase", [], None, "{tracks?, ticks: bool}", has_selection, |e, p| flag(
            e,
            p,
            "track.timebase",
            |t, v| t.ticks_timebase = v,
            |t| t.ticks_timebase
        )),
        cmd!(
            "track.elastic",
            "Elastic Audio",
            [],
            None,
            "{tracks?, algorithm?: polyphonic|rhythmic|monophonic|varispeed|x-form|none}",
            has_selection,
            |e, p| {
                let tracks = tracks_required(e, "track.elastic", p)?;
                let alg = str_param(p, "algorithm").filter(|a| *a != "none").map(str::to_string);
                let s = e.session_mut();
                for t in tracks {
                    if let Some(tr) = s.track_mut(t) {
                        tr.elastic = alg.clone();
                    }
                }
                Ok(json!({"algorithm": alg}))
            }
        ),
    ]
}

fn new_track(e: &mut Engine, p: &Value) -> Result<Value> {
    let count = i64_or(p, "count", 1).clamp(1, 128);
    let kind = match str_param(p, "kind") {
        Some(k) => TrackKind::from_id(k).ok_or_else(|| bad("track.new", format!("unknown kind `{k}`")))?,
        None => TrackKind::Audio,
    };
    let default_fmt = if matches!(kind, TrackKind::Master | TrackKind::Aux | TrackKind::Instrument | TrackKind::Folder) {
        ChannelFormat::Stereo
    } else {
        ChannelFormat::Mono
    };
    let format = match str_param(p, "format") {
        Some(f) => ChannelFormat::from_id(f).ok_or_else(|| bad("track.new", format!("unknown format `{f}`")))?,
        None => default_fmt,
    };
    let name = str_param(p, "name").map(str::to_string);
    let ticks = str_param(p, "timebase").map(|t| t == "ticks");
    let instrument = str_param(p, "instrument").map(str::to_string);
    let s = e.session_mut();
    let mut ids = Vec::new();
    for _ in 0..count {
        let id = s.add_track(kind, format, name.as_deref());
        if let Some(t) = s.track_mut(id) {
            if let Some(tb) = ticks {
                t.ticks_timebase = tb;
            }
            if kind == TrackKind::Instrument {
                t.instrument = Some(soundcraft_model::Insert::new(instrument.clone().unwrap_or_else(|| "subtractive_synth".into())));
                // Plays whatever MIDI keyboard is plugged in once armed.
                t.mixer.input = Route::Midi(soundcraft_model::MidiIn::all());
            }
        }
        if kind == TrackKind::Folder {
            let folder_name = s.track(id).map(|track| track.name.clone()).unwrap_or_else(|| "Folder".into());
            let bus_name = unique_folder_bus_name(s, &format!("{folder_name} Bus"), None);
            let bus = s.add_bus(&bus_name, format);
            if let Some(track) = s.track_mut(id) {
                track.mixer.input = Route::Bus(bus);
            }
        }
        ids.push(id);
    }
    s.edit.selected_tracks = ids.clone();
    Ok(json!({"tracks": ids}))
}

fn group(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.group", p)?;
    let s = e.session_mut();
    let id = GroupId(s.alloc());
    let letter = (b'a'..=b'z').map(char::from).find(|c| !s.groups.iter().any(|g| g.letter == *c)).unwrap_or('z');
    let name = str_param(p, "name").map_or_else(|| format!("Group {}", s.groups.len() + 1), str::to_string);
    let color = s.next_track_color();
    s.groups.push(Group {
        id,
        name,
        letter,
        members: tracks,
        edit: bool_or(p, "edit", true),
        mix: bool_or(p, "mix", true),
        active: true,
        color,
        attributes: vec!["volume".into(), "mute".into(), "solo".into(), "record".into()],
    });
    Ok(json!({"group": id}))
}

fn find_group(e: &Engine, p: &Value, cmd: &str) -> Result<GroupId> {
    let s = e.session();
    let g = match (p.get("group").and_then(Value::as_u64), str_param(p, "group")) {
        (Some(n), _) => s.groups.iter().find(|g| g.id.0 == n),
        (_, Some(name)) => s.groups.iter().find(|g| g.name == name || g.letter.to_string() == name),
        _ => None,
    };
    g.map(|g| g.id).ok_or_else(|| bad(cmd, "unknown `group`"))
}

fn delete_group(e: &mut Engine, p: &Value) -> Result<Value> {
    let g = find_group(e, p, "track.ungroup")?;
    e.session_mut().groups.retain(|x| x.id != g);
    Ok(json!({}))
}

fn toggle_group(e: &mut Engine, p: &Value) -> Result<Value> {
    let g = find_group(e, p, "track.group_toggle")?;
    let s = e.session_mut();
    let mut active = false;
    if let Some(gr) = s.groups.iter_mut().find(|x| x.id == g) {
        gr.active = !gr.active;
        active = gr.active;
    }
    Ok(json!({"active": active}))
}

/// Change an existing group's name, membership and Edit/Mix linking. Omitted fields keep their value.
fn modify_group(e: &mut Engine, p: &Value) -> Result<Value> {
    const CMD: &str = "track.group_modify";
    let g = find_group(e, p, CMD)?;
    // Absent `tracks` leaves membership alone; present, it replaces it. Only an array counts, so a
    // mistyped param can't silently swap the members for whatever happens to be selected.
    let members = match p.get("tracks") {
        None => None,
        Some(Value::Array(_)) => {
            let mut t = tracks_param(e, CMD, p)?;
            // A track listed twice would take every linked move twice over.
            let mut seen = std::collections::HashSet::new();
            t.retain(|id| seen.insert(*id));
            if t.is_empty() {
                return Err(bad(CMD, "a group needs at least one track"));
            }
            Some(t)
        }
        Some(_) => return Err(bad(CMD, "`tracks` must be an array of track ids or names")),
    };
    let name = match str_param(p, "name") {
        None => None,
        Some(n) => {
            let n = n.trim();
            if n.is_empty() {
                return Err(bad(CMD, "`name` must not be empty"));
            }
            // `find_group` resolves by name, so duplicates would make a group unaddressable.
            if e.session().groups.iter().any(|x| x.name == n && x.id != g) {
                return Err(bad(CMD, format!("a group named `{n}` already exists")));
            }
            let mut n = n.to_string();
            soundcraft_model::truncate_chars(&mut n, soundcraft_model::MAX_NAME_CHARS);
            Some(n)
        }
    };
    let s = e.session_mut();
    let Some(gr) = s.groups.iter_mut().find(|x| x.id == g) else { return Err(bad(CMD, "unknown `group`")) };
    if let Some(n) = name {
        gr.name = n;
    }
    if let Some(m) = members {
        gr.members = m;
    }
    gr.edit = bool_or(p, "edit", gr.edit);
    gr.mix = bool_or(p, "mix", gr.mix);
    Ok(json!({"group": g, "name": gr.name, "tracks": gr.members, "edit": gr.edit, "mix": gr.mix}))
}

fn duplicate(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.duplicate", p)?;
    let (pl, auto, ins, snd) =
        (bool_or(p, "playlists", true), bool_or(p, "automation", true), bool_or(p, "inserts", true), bool_or(p, "sends", true));
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in tracks {
        let Some(idx) = s.track_index(t) else { continue };
        let Some(mut nt) = s.tracks.get(idx).cloned() else { continue };
        nt.id = soundcraft_model::TrackId(s.alloc());
        nt.name = s.unique_track_name(&format!("{}.dup", nt.name));
        if !pl {
            for p in &mut nt.playlists {
                p.clips.clear();
            }
        } else {
            for p in &mut nt.playlists {
                for c in &mut p.clips {
                    c.id = s.new_clip_id();
                }
            }
        }
        if !auto {
            nt.automation.clear();
        }
        if !ins {
            nt.mixer.inserts.clear();
        }
        if !snd {
            nt.mixer.sends.iter_mut().for_each(|i| *i = None);
        }
        // A copy without the clips, automation or modules its frozen render was made from plays
        // live (the render stays with the original).
        if !(pl && auto && ins) {
            nt.frozen = false;
            nt.freeze = None;
        }
        out.push(nt.id);
        s.tracks.insert((idx + 1).min(s.tracks.len()), nt);
    }
    s.edit.selected_tracks = out.clone();
    Ok(json!({"tracks": out}))
}

fn split_mono(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.split_into_mono", p)?;
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in tracks {
        let Some(src) = s.track(t).cloned() else { continue };
        if src.channels() < 2 || src.kind != TrackKind::Audio {
            continue;
        }
        for ch in 0..src.channels() {
            let suffix = match (src.channels(), ch) {
                (2, 0) => "L".to_string(),
                (2, 1) => "R".to_string(),
                _ => format!("{}", ch + 1),
            };
            let id = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("{}.{suffix}", src.name)));
            let mut clips = src.clips().to_vec();
            for c in &mut clips {
                c.id = s.new_clip_id();
                // Mono tracks read channel 0 of the source; record the channel in the name for now.
                c.name = format!("{}.{suffix}", c.name);
            }
            if let Some(tr) = s.track_mut(id) {
                tr.mixer.pan = vec![if ch == 0 { -1.0 } else { 1.0 }];
                tr.comments = format!("source_channel={ch}");
                if let Some(pl) = tr.playlist_mut() {
                    pl.clips = clips;
                }
            }
            out.push(id);
        }
    }
    Ok(json!({"tracks": out}))
}

fn flag(
    e: &mut Engine,
    p: &Value,
    cmd: &str,
    set: fn(&mut soundcraft_model::Track, bool),
    get: fn(&soundcraft_model::Track) -> bool,
) -> Result<Value> {
    let tracks = tracks_required(e, cmd, p)?;
    let explicit = p.get("inactive").or_else(|| p.get("hidden")).or_else(|| p.get("value")).or_else(|| p.get("ticks")).and_then(Value::as_bool);
    let s = e.session_mut();
    let target = explicit.unwrap_or_else(|| !tracks.iter().all(|t| s.track(*t).is_some_and(get)));
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            set(tr, target);
        }
    }
    Ok(json!({"value": target, "tracks": tracks}))
}

/// A routing folder owns its summing bus; never reuse a user bus with the same name.
fn unique_folder_bus_name(s: &Session, base: &str, own: Option<soundcraft_model::BusId>) -> String {
    for suffix in 1..=s.busses.len().saturating_add(2) {
        let candidate = if suffix == 1 { base.to_string() } else { format!("{base} {suffix}") };
        if !s.busses.iter().any(|bus| bus.name == candidate && Some(bus.id) != own) {
            return candidate;
        }
    }
    format!("{base} {}", s.busses.len().saturating_add(3))
}

fn move_to_folder(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.move_to_new_folder", p)?;
    let name = str_param(p, "name").map(str::to_string);
    let s = e.session_mut();
    let first_idx = tracks.iter().filter_map(|t| s.track_index(*t)).min().unwrap_or(0);
    let fid = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, Some(name.as_deref().unwrap_or("Folder")));
    // Place the folder above its first member.
    if let Some(i) = s.track_index(fid) {
        let f = s.tracks.remove(i);
        s.tracks.insert(first_idx.min(s.tracks.len()), f);
    }
    // A routing folder sums its members through its own bus.
    let fname = s.track(fid).map(|x| x.name.clone()).unwrap_or_else(|| "Folder".into());
    let bus_name = unique_folder_bus_name(s, &format!("{fname} Bus"), None);
    let bus = s.add_bus(&bus_name, ChannelFormat::Stereo);
    if let Some(f) = s.track_mut(fid) {
        f.mixer.input = Route::Bus(bus);
    }
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.folder = Some(fid);
            if tr.mixer.output == Route::Main {
                tr.mixer.output = Route::Bus(bus);
            }
        }
    }
    place_under(s, fid, &tracks);
    Ok(json!({"folder": fid, "bus": bus}))
}

/// Whether `track` sits inside `folder`, directly or through nested folders.
fn in_folder(s: &Session, track: TrackId, folder: TrackId) -> bool {
    let mut current = s.track(track).and_then(|track| track.folder);
    // The depth cap guards against a cyclic folder chain in a hostile session file.
    for _ in 0..64 {
        match current {
            Some(parent) if parent == folder => return true,
            Some(parent) => current = s.track(parent).and_then(|track| track.folder),
            None => return false,
        }
    }
    false
}

/// Keep a folder's members together immediately below it, preserving their relative order.
fn place_under(s: &mut Session, folder: TrackId, tracks: &[TrackId]) {
    // Moving a folder also moves its complete subtree as one stable block.
    let moved: Vec<_> = s
        .tracks
        .iter()
        .filter(|track| track.id != folder && (tracks.contains(&track.id) || tracks.iter().any(|root| in_folder(s, track.id, *root))))
        .cloned()
        .collect();
    let moved_ids: Vec<_> = moved.iter().map(|track| track.id).collect();
    s.tracks.retain(|track| !moved_ids.contains(&track.id));
    let ids: Vec<TrackId> = s.tracks.iter().map(|track| track.id).collect();
    let Some(folder_index) = ids.iter().position(|track| *track == folder) else { return };
    let mut insert_at = folder_index + 1;
    while ids.get(insert_at).is_some_and(|track| in_folder(s, *track, folder)) {
        insert_at += 1;
    }
    for (offset, track) in moved.into_iter().enumerate() {
        s.tracks.insert((insert_at + offset).min(s.tracks.len()), track);
    }
}

fn folder_bus(s: &Session, folder: TrackId) -> Option<Route> {
    s.track(folder).map(|folder| folder.mixer.input.clone()).filter(|route| matches!(route, Route::Bus(_)))
}

/// The summing bus owned by a routing folder.
fn owned_folder_bus(s: &Session, folder: TrackId) -> Option<soundcraft_model::BusId> {
    let folder = s.track(folder).filter(|track| track.kind == TrackKind::Folder)?;
    match folder.mixer.input {
        Route::Bus(id) => Some(id),
        _ => None,
    }
}

/// Keep only the selected roots so selecting a folder and one of its children preserves the subtree.
fn root_tracks(s: &Session, tracks: Vec<TrackId>) -> Vec<TrackId> {
    tracks.iter().copied().filter(|track| !tracks.iter().any(|root| root != track && in_folder(s, *track, *root))).collect()
}

fn move_into_folder(e: &mut Engine, p: &Value) -> Result<Value> {
    let folder = track_param(e, "track.move_to_folder", p, "folder")?.ok_or_else(|| bad("track.move_to_folder", "`folder` required"))?;
    let tracks = tracks_required(e, "track.move_to_folder", p)?;
    let session = e.session_mut();
    if !session.track(folder).is_some_and(Track::is_folder) {
        return Err(bad("track.move_to_folder", "`folder` is not a folder track"));
    }
    // A folder cannot go inside itself or one of its descendants.
    let tracks: Vec<TrackId> = tracks.into_iter().filter(|track| *track != folder && !in_folder(session, folder, *track)).collect();
    let tracks = root_tracks(session, tracks);
    if tracks.is_empty() {
        return Err(bad("track.move_to_folder", "nothing to move into that folder"));
    }
    let bus = folder_bus(session, folder);
    for track in &tracks {
        let old_bus = session.track(*track).and_then(|track| track.folder).and_then(|parent| folder_bus(session, parent));
        if let Some(moved) = session.track_mut(*track) {
            moved.folder = Some(folder);
            if let Some(bus) = &bus
                && (moved.mixer.output == Route::Main || Some(&moved.mixer.output) == old_bus.as_ref())
            {
                moved.mixer.output = bus.clone();
            }
        }
    }
    place_under(session, folder, &tracks);
    Ok(json!({"folder": folder, "moved": tracks.len()}))
}

fn remove_from_folder(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.remove_from_folder", p)?;
    let session = e.session_mut();
    let tracks = root_tracks(session, tracks);
    let mut removed = 0;
    for track in &tracks {
        let Some(folder) = session.track(*track).and_then(|track| track.folder) else { continue };
        let bus = folder_bus(session, folder);
        let parent = session.track(folder).and_then(|folder| folder.folder);
        let parent_bus = parent.and_then(|parent| folder_bus(session, parent));
        if let Some(moved) = session.track_mut(*track) {
            moved.folder = parent;
            if bus.as_ref() == Some(&moved.mixer.output) {
                moved.mixer.output = parent_bus.unwrap_or(Route::Main);
            }
            removed += 1;
        }
        let moved: Vec<_> =
            session.tracks.iter().filter(|candidate| candidate.id == *track || in_folder(session, candidate.id, *track)).cloned().collect();
        let moved_ids: Vec<_> = moved.iter().map(|candidate| candidate.id).collect();
        session.tracks.retain(|candidate| !moved_ids.contains(&candidate.id));
        if let Some(folder_index) = session.track_index(folder) {
            let mut insert_at = folder_index + 1;
            while session.tracks.get(insert_at).is_some_and(|candidate| in_folder(session, candidate.id, folder)) {
                insert_at += 1;
            }
            for (offset, moved) in moved.into_iter().enumerate() {
                session.tracks.insert((insert_at + offset).min(session.tracks.len()), moved);
            }
        }
    }
    Ok(json!({"removed": removed}))
}

fn change_width(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.change_width", p)?;
    let f = str_param(p, "format").and_then(ChannelFormat::from_id).ok_or_else(|| bad("track.change_width", "`format` required"))?;
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.format = f;
            tr.mixer.pan = soundcraft_model::Mixer::new(f).pan;
        }
    }
    Ok(json!({"format": f}))
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.delete", p)?;
    let s = e.session_mut();
    let renders: Vec<_> = tracks.iter().filter_map(|t| s.track(*t).and_then(|t| t.freeze)).map(|f| f.source).collect();
    let removed_busses: Vec<_> = tracks
        .iter()
        .filter_map(|folder| {
            let bus = owned_folder_bus(s, *folder)?;
            let mut parent = s.track(*folder).and_then(|folder| folder.folder);
            for _ in 0..64 {
                match parent {
                    Some(candidate) if tracks.contains(&candidate) => parent = s.track(candidate).and_then(|folder| folder.folder),
                    _ => break,
                }
            }
            Some((bus, parent.and_then(|parent| folder_bus(s, parent)).unwrap_or(Route::Main)))
        })
        .collect();
    // Promote children of deleted folders and repair only routes that used the deleted
    // folder's own bus. Custom outputs remain untouched.
    let mut promoted = Vec::new();
    for child in &s.tracks {
        if tracks.contains(&child.id) {
            continue;
        }
        let Some(folder) = child.folder.filter(|folder| tracks.contains(folder)) else { continue };
        let mut parent = s.track(folder).and_then(|folder| folder.folder);
        for _ in 0..64 {
            match parent {
                Some(candidate) if tracks.contains(&candidate) => parent = s.track(candidate).and_then(|folder| folder.folder),
                _ => break,
            }
        }
        promoted.push((child.id, folder_bus(s, folder), parent, parent.and_then(|parent| folder_bus(s, parent))));
    }
    for (child, old_bus, parent, parent_bus) in promoted {
        if let Some(child) = s.track_mut(child) {
            child.folder = parent;
            if old_bus.as_ref() == Some(&child.mixer.output) {
                child.mixer.output = parent_bus.unwrap_or(Route::Main);
            }
        }
    }
    s.tracks.retain(|t| !tracks.contains(&t.id));
    s.edit.selected_tracks.retain(|t| !tracks.contains(t));
    for g in &mut s.groups {
        g.members.retain(|t| !tracks.contains(t));
    }
    for (bus, replacement) in removed_busses {
        s.busses.retain(|candidate| candidate.id != bus);
        for track in &mut s.tracks {
            if track.mixer.input == Route::Bus(bus) {
                track.mixer.input = Route::None;
            }
            if track.mixer.output == Route::Bus(bus) {
                track.mixer.output = replacement.clone();
            }
            for send in &mut track.mixer.sends {
                if send.as_ref().is_some_and(|send| send.target == Route::Bus(bus)) {
                    *send = None;
                }
            }
        }
    }
    // A deleted frozen track's render goes with it (the file stays on disk; undo brings it back).
    for src in renders {
        super::freeze::drop_unused_source(s, src);
    }
    Ok(json!({"deleted": tracks.len()}))
}

fn rename(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, "track.rename", p, "track")?.ok_or_else(|| bad("track.rename", "`track` required"))?;
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("track.rename", "`name` required"))?;
    let s = e.session_mut();
    if s.tracks.iter().any(|x| x.name == name && x.id != t) {
        return Err(bad("track.rename", format!("a track named `{name}` already exists")));
    }
    let name: String = name.chars().take(64).collect();
    let bus = s.track(t).and_then(|track| match (track.kind, &track.mixer.input) {
        (TrackKind::Folder, Route::Bus(bus)) => Some(*bus),
        _ => None,
    });
    if let Some(track) = s.track_mut(t) {
        track.name.clone_from(&name);
    }
    if let Some(bus) = bus {
        let bus_name = unique_folder_bus_name(s, &format!("{name} Bus"), Some(bus));
        if let Some(bus) = s.busses.iter_mut().find(|candidate| candidate.id == bus) {
            bus.name = bus_name;
        }
    }
    Ok(json!({"name": name}))
}

fn comments(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, "track.comments", p, "track")?.ok_or_else(|| bad("track.comments", "`track` required"))?;
    let c = str_param(p, "comments").unwrap_or_default().to_string();
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.comments = c;
    }
    Ok(json!({}))
}

fn color(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.color", p)?;
    let c = match p.get("color") {
        Some(Value::Array(a)) if a.len() == 3 => {
            let v: Vec<u8> = a.iter().map(|x| x.as_u64().unwrap_or(0).min(255) as u8).collect();
            [v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0), v.get(2).copied().unwrap_or(0)]
        }
        Some(v) if v.is_u64() => soundcraft_model::TRACK_COLORS.get(v.as_u64().unwrap_or(0) as usize % 16).copied().unwrap_or([128, 128, 128]),
        _ => return Err(bad("track.color", "`color` must be [r,g,b] or a palette index")),
    };
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.color = c;
        }
    }
    Ok(json!({"color": c}))
}

fn height(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.height", p)?;
    let h = match p.get("height").and_then(Value::as_f64) {
        Some(points) => TrackHeight::from_points(points as f32),
        None => str_param(p, "height").and_then(TrackHeight::from_id).ok_or_else(|| bad("track.height", "unknown height"))?,
    };
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            tr.height = h;
        }
    }
    Ok(json!({"height": h}))
}

fn height_step(e: &mut Engine, p: &Value, taller: bool) -> Result<Value> {
    let tracks = tracks_required(e, if taller { "track.height_up" } else { "track.height_down" }, p)?;
    let mut steps = Vec::new();
    for tr in tracks.iter().filter_map(|t| e.session().track(*t)) {
        let h = tr.height.step(taller);
        if h != tr.height {
            steps.push((tr.id, h));
        }
    }
    if !steps.is_empty() {
        let s = e.session_mut();
        for (t, h) in &steps {
            if let Some(tr) = s.track_mut(*t) {
                tr.height = *h;
            }
        }
    }
    Ok(json!({"changed": steps.len()}))
}

fn view(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.view", p)?;
    let v = str_param(p, "view").ok_or_else(|| bad("track.view", "`view` required"))?.to_string();
    let plain = matches!(v.as_str(), "waveform" | "pitch" | "blocks" | "notes" | "regions" | "clip_gain" | "playlists");
    // Automation views are stored by text id; a legacy chain position becomes the module's id.
    let mut views = Vec::new();
    for t in &tracks {
        let Some(tr) = e.session().track(*t) else { continue };
        let view = if plain {
            v.clone()
        } else {
            match super::mix::resolve_auto_param(tr, &v) {
                Some(soundcraft_model::AutoParam::Plugin { module, param })
                    if tr.mixer.module(module).is_some_and(|ins| super::mix::is_link_role_param(&ins.plugin, &param)) =>
                {
                    return Err(bad(
                        "track.view",
                        format!("`{param}` is a link role: it sets the rendering order of its link group and is not automatable"),
                    ));
                }
                Some(p) => p.id(),
                None => return Err(bad("track.view", format!("unknown view `{v}`"))),
            }
        };
        views.push((*t, view));
    }
    let s = e.session_mut();
    for (t, view) in &views {
        if let Some(tr) = s.track_mut(*t) {
            tr.view = view.clone();
        }
    }
    Ok(json!({"view": v}))
}

/// The lane id `v` names on `tr`, refused like `track.view` when it does not resolve or is a
/// link role.
fn lane_id(cmd: &str, tr: &soundcraft_model::Track, v: &str) -> Result<String> {
    match super::mix::resolve_auto_param(tr, v) {
        Some(soundcraft_model::AutoParam::Plugin { module, param })
            if tr.mixer.module(module).is_some_and(|ins| super::mix::is_link_role_param(&ins.plugin, &param)) =>
        {
            Err(bad(cmd, format!("`{param}` is a link role: it sets the rendering order of its link group and is not automatable")))
        }
        Some(p) => Ok(p.id()),
        None => Err(bad(cmd, format!("unknown automation lane `{v}`"))),
    }
}

fn auto_lane(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.auto_lane";
    let tracks = tracks_required(e, C, p)?;
    let v = str_param(p, "param").ok_or_else(|| bad(C, "`param` required"))?.to_string();
    let show = p.get("show").and_then(Value::as_bool);
    let mut plan = Vec::new();
    for t in &tracks {
        let Some(tr) = e.session().track(*t) else { continue };
        let id = lane_id(C, tr, &v)?;
        let after = match str_param(p, "after") {
            Some(a) => Some(lane_id(C, tr, a)?),
            None => None,
        };
        plan.push((*t, id, after));
    }
    let first = plan.first().and_then(|(t, id, _)| e.session().track(*t).map(|tr| tr.auto_lanes.contains(id)));
    let want = show.unwrap_or(!first.unwrap_or(false));
    let s = e.session_mut();
    for (t, id, after) in plan {
        let Some(tr) = s.track_mut(t) else { continue };
        tr.auto_lanes.retain(|x| *x != id);
        if want {
            let at = after.and_then(|a| tr.auto_lanes.iter().position(|x| *x == a)).map_or(tr.auto_lanes.len(), |i| i + 1);
            tr.auto_lanes.insert(at, id);
        }
    }
    Ok(json!({"shown": want}))
}

fn auto_lanes(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.auto_lanes";
    let tracks = tracks_required(e, C, p)?;
    let show = str_param(p, "show").ok_or_else(|| bad(C, "`show` required: data or none"))?;
    if !matches!(show, "data" | "none") {
        return Err(bad(C, format!("`show` must be data or none, not `{show}`")));
    }
    let s = e.session_mut();
    for t in tracks {
        let Some(tr) = s.track_mut(t) else { continue };
        if show == "none" {
            tr.auto_lanes.clear();
            continue;
        }
        let mut ids: Vec<String> = Vec::new();
        for l in tr.automation.iter().filter(|l| !l.points.is_empty()) {
            let ok = match &l.param {
                soundcraft_model::AutoParam::Plugin { module, .. } => tr.mixer.module(*module).is_some(),
                _ => true,
            };
            let id = l.param.id();
            if ok && id != tr.view && !ids.contains(&id) {
                ids.push(id);
            }
        }
        if ids.is_empty() && tr.view != "volume" {
            ids.push("volume".into());
        }
        // Lanes already shown keep their place; new ones go below them.
        let mut lanes: Vec<String> = tr.auto_lanes.clone();
        lanes.extend(ids.into_iter().filter(|i| !tr.auto_lanes.contains(i)));
        tr.auto_lanes = lanes;
    }
    Ok(json!({}))
}

fn move_track(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, "track.move", p, "track")?.ok_or_else(|| bad("track.move", "`track` required"))?;
    let to = i64_or(p, "to", 0).max(0) as usize;
    let count = i64_or(p, "count", 1).max(1) as usize;
    let s = e.session_mut();
    if let Some(i) = s.track_index(t) {
        let end = i.saturating_add(count).min(s.tracks.len());
        let block: Vec<_> = s.tracks.drain(i..end).collect();
        let at = to.min(s.tracks.len());
        s.tracks.splice(at..at, block);
    }
    Ok(json!({}))
}

fn commit(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.commit", p)?;
    let mut out = Vec::new();
    for t in tracks {
        if let Some(id) = crate::io::commit_track(e, t)? {
            out.push(id);
        }
    }
    Ok(json!({"tracks": out}))
}

fn bypass_inserts(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.bypass_inserts", p)?;
    let s = e.session_mut();
    let target = p
        .get("bypass")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| !tracks.iter().all(|t| s.track(*t).is_some_and(|tr| tr.mixer.inserts.iter().skip(tr.frozen_modules()).all(|i| i.bypass))));
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            // A frozen track's frozen modules are left as they are: only the live ones switch.
            let live = tr.frozen_modules();
            for i in tr.mixer.inserts.iter_mut().skip(live) {
                i.bypass = target;
            }
        }
    }
    Ok(json!({"bypass": target}))
}

fn mute_sends(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.mute_sends", p)?;
    let s = e.session_mut();
    let target = p.get("mute").and_then(Value::as_bool).unwrap_or(true);
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            for snd in tr.mixer.sends.iter_mut().flatten() {
                snd.mute = target;
            }
        }
    }
    Ok(json!({"mute": target}))
}

fn create_click(e: &mut Engine, _: &Value) -> Result<Value> {
    let s = e.session_mut();
    let id = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("Click"));
    if let Some(t) = s.track_mut(id) {
        t.comments = "click".into();
    }
    s.edit.click = true;
    Ok(json!({"track": id}))
}

fn new_playlist(e: &mut Engine, p: &Value, dup: bool) -> Result<Value> {
    let tracks = tracks_required(e, "track.playlist_new", p)?;
    let name = str_param(p, "name").map(str::to_string);
    let s = e.session_mut();
    for t in &tracks {
        let mut clips = Vec::new();
        if dup && let Some(tr) = s.track(*t) {
            clips = tr.clips().to_vec();
        }
        for c in &mut clips {
            c.id = s.new_clip_id();
        }
        if let Some(tr) = s.track_mut(*t) {
            let n = name.clone().unwrap_or_else(|| format!("{}.{:02}", tr.name, tr.playlists.len() + 1));
            tr.playlists.push(Playlist { name: n, clips, take_color: None });
            tr.active_playlist = tr.playlists.len() - 1;
        }
    }
    Ok(json!({"tracks": tracks}))
}

fn select_playlist(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, "track.playlist_select", p)?;
    let idx = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    let name = str_param(p, "name").map(str::to_string);
    let s = e.session_mut();
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            let i = idx.or_else(|| name.as_ref().and_then(|n| tr.playlists.iter().position(|p| &p.name == n)));
            match i {
                Some(i) if i < tr.playlists.len() => tr.active_playlist = i,
                _ => return Err(bad("track.playlist_select", "no such playlist")),
            }
        }
    }
    Ok(json!({}))
}

/// A route by name. Named paths from the I/O setup come before busses, so they never become a
/// bus: a session input (for inputs) is `Hardware(<its stored name>)`; for outputs the main
/// output's name is `Main` and another output's is `Hardware(<its stored name>)`.
pub(crate) fn parse_route(s: &soundcraft_model::Session, v: &str, input: bool) -> Route {
    // Reserved names win over everything (setup.io refuses them for paths).
    if let Some(r) = Route::reserved(v) {
        return r;
    }
    if input && let Some(Ok(n)) = musician_route(v) {
        return Route::Musician(n);
    }
    if input && let Some(Ok(r)) = module_route(s, v) {
        return Route::Modules(vec![r]);
    }
    if let Some(name) = named_path(s, v, input) {
        return if !input && s.output_index(v) == Some(0) { Route::Main } else { Route::Hardware(name.to_string()) };
    }
    s.bus_by_name(v).map_or_else(|| Route::Hardware(v.to_string()), |b| Route::Bus(b.id))
}

/// Hardware MIDI input named as a route: `midi:all` (or `MIDI: All Inputs`, `midi:`) for every
/// port, `midi:<port name>` (or `MIDI: <port name>`) for one; Omni. `None` when `v` doesn't start
/// with `midi:` (case-insensitive).
pub(crate) fn midi_route(v: &str) -> Option<soundcraft_model::MidiIn> {
    let t = v.trim();
    let rest = t.get(..5).filter(|h| h.eq_ignore_ascii_case("midi:")).and_then(|_| t.get(5..))?.trim();
    let all = rest.is_empty() || rest.eq_ignore_ascii_case("all") || rest.eq_ignore_ascii_case("all inputs");
    Some(soundcraft_model::MidiIn { device: (!all).then(|| rest.to_string()), channel: 0 })
}

/// A Band musician named as a route: `musician:N` or `Musician N` (case-insensitive), N 1..=6.
/// `None` when `v` doesn't name a musician; `Some(Ok(slot))` with the 0-based slot; `Some(Err)`
/// when it does but N is not 1..=6.
pub(crate) fn musician_route(v: &str) -> Option<std::result::Result<u8, String>> {
    let t = v.trim();
    let head = t.get(..8)?;
    if !head.eq_ignore_ascii_case("musician") {
        return None;
    }
    let rest = t.get(8..)?;
    let num = rest.strip_prefix(':').or_else(|| rest.strip_prefix(' '))?.trim();
    match num.parse::<u8>() {
        Ok(n @ 1..=6) => Some(Ok(n - 1)),
        _ => Some(Err(format!("`{t}`: musicians are 1-6"))),
    }
}

/// A chain module's MIDI output named as a route: `module:<track>/<module>` (case-insensitive
/// prefix), the track by id or name and the module by its 1-based chain position, its id or its
/// plugin's name or id (the first match in chain order). `None` when `v` doesn't name a module;
/// `Some(Err)` when it does but the track or module isn't there, or the module has no MIDI
/// output; `Some(Ok(the module))` otherwise.
pub(crate) fn module_route(s: &soundcraft_model::Session, v: &str) -> Option<std::result::Result<ModuleRef, String>> {
    let t = v.trim();
    let rest = t.get(..7).filter(|h| h.eq_ignore_ascii_case("module:")).and_then(|_| t.get(7..))?;
    let Some((track, module)) = rest.rsplit_once('/') else {
        return Some(Err(format!("`{t}`: a module input is `module:<track>/<module>`")));
    };
    let (track, module) = (track.trim(), module.trim());
    let Some(tr) = track.parse::<u64>().ok().and_then(|n| s.track(TrackId(n))).or_else(|| s.track_by_name(track)) else {
        return Some(Err(format!("no track named `{track}`")));
    };
    let by_index = module.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| tr.mixer.inserts.get(i));
    let by_id = module.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()).and_then(|n| tr.mixer.module(soundcraft_model::ModuleId(n)));
    let by_name = || {
        tr.mixer.inserts.iter().find(|i| {
            i.plugin.eq_ignore_ascii_case(module) || soundcraft_dsp::plugin_info(&i.plugin).is_some_and(|p| p.name.eq_ignore_ascii_case(module))
        })
    };
    let Some(ins) = by_index.or(by_id).or_else(by_name) else {
        return Some(Err(format!("`{}` has no module `{module}`", tr.name)));
    };
    let info = soundcraft_dsp::plugin_info(&ins.plugin);
    if !info.is_some_and(|p| p.midi_out) {
        let name = info.map_or(ins.plugin.as_str(), |p| p.name);
        return Some(Err(format!("{name} has no MIDI output")));
    }
    Some(Ok(ModuleRef { track: tr.id, module: ins.id }))
}

/// `input` as a list of module routes: a JSON array of `module:<track>/<module>` names, or one
/// such string. `None` when `p[key]` is neither; `Some(Err)` for an array holding anything else,
/// a bad module name, or an empty array.
fn module_list(s: &soundcraft_model::Session, p: &Value, key: &str) -> Option<std::result::Result<Vec<ModuleRef>, String>> {
    let v = p.get(key)?;
    if let Some(one) = v.as_str() {
        return module_route(s, one).map(|r| r.map(|m| vec![m]));
    }
    let arr = v.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(name) = item.as_str() else {
            return Some(Err(format!("`{key}`: a list holds module names (`module:<track>/<module>`), not {item}")));
        };
        match module_route(s, name) {
            Some(Ok(m)) => {
                if !out.contains(&m) {
                    out.push(m);
                }
            }
            Some(Err(e)) => return Some(Err(e)),
            None => return Some(Err(format!("`{name}`: a list of inputs holds module names (`module:<track>/<module>`)"))),
        }
    }
    if out.is_empty() {
        return Some(Err(format!("`{key}`: an empty list; use `none` for no input")));
    }
    Some(Ok(out))
}

/// The stored name of the session input (or output) called `v`, case-insensitively.
fn named_path<'a>(s: &'a soundcraft_model::Session, v: &str, input: bool) -> Option<&'a str> {
    if input {
        s.inputs.iter().find(|i| soundcraft_model::same_name(&i.name, v)).map(|i| i.name.as_str())
    } else {
        s.output_index(v).and_then(|i| s.outputs.get(i)).map(|o| o.name.as_str())
    }
}

fn route(e: &mut Engine, p: &Value, input: bool) -> Result<Value> {
    let key = if input { "input" } else { "output" };
    let tracks = tracks_required(e, "track.route", p)?;
    let create_bus = bool_or(p, "create_bus", true);
    let cmd = if input { "track.input" } else { "track.output" };
    // Modules' MIDI: one name or a list of them, into MIDI and instrument tracks only.
    match module_list(e.session(), p, key) {
        Some(_) if !input => return Err(bad(cmd, "A module's MIDI can only be a track input")),
        Some(Err(m)) => return Err(bad(cmd, m)),
        Some(Ok(refs)) => {
            if let Some(t) = tracks.iter().filter_map(|t| e.session().track(*t)).find(|t| !t.kind.is_midi()) {
                return Err(bad(cmd, format!("A module's MIDI plays into a MIDI or instrument track (`{}` is not one)", t.name)));
            }
            if refs.iter().any(|r| tracks.contains(&r.track)) {
                return Err(bad(cmd, "That would feed a track into itself"));
            }
            // A loop through busses, sidechains or other module inputs: the mix engine can't
            // order a source before the track.
            let route = Route::Modules(refs);
            for t in &tracks {
                if let Some(why) = soundcraft_mix::module_input_refusal(e.session(), *t, &route) {
                    return Err(bad(cmd, why));
                }
            }
            let s = e.session_mut();
            for t in &tracks {
                if let Some(tr) = s.track_mut(*t) {
                    tr.mixer.input = route.clone();
                }
            }
            return Ok(json!({key: route}));
        }
        None => {}
    }
    let v = str_param(p, key).ok_or_else(|| bad("track.route", format!("`{key}` required")))?.to_string();
    if let Some(mut m) = midi_route(&v) {
        if !input {
            return Err(bad(cmd, "MIDI input can only be a track input"));
        }
        if let Some(t) = tracks.iter().filter_map(|t| e.session().track(*t)).find(|t| !t.kind.is_midi()) {
            return Err(bad(cmd, format!("MIDI input plays into a MIDI or instrument track (`{}` is not one)", t.name)));
        }
        if let Some(c) = p.get("channel") {
            m.channel = match c.as_u64().or_else(|| c.as_str().filter(|x| x.eq_ignore_ascii_case("omni")).map(|_| 0)) {
                Some(n @ 0..=16) => n as u8,
                _ => return Err(bad(cmd, "`channel` is 0 (Omni) or 1-16")),
            };
        }
        let r = Route::Midi(m);
        let s = e.session_mut();
        for t in &tracks {
            if let Some(tr) = s.track_mut(*t) {
                tr.mixer.input = r.clone();
            }
        }
        return Ok(json!({key: r}));
    }
    match musician_route(&v) {
        Some(_) if !input => return Err(bad(cmd, "A musician can only be a track input")),
        Some(Err(m)) => return Err(bad(cmd, m)),
        Some(Ok(_)) => {
            // The band's events need an instrument to play them.
            if let Some(t) = tracks.iter().filter_map(|t| e.session().track(*t)).find(|t| !t.kind.is_midi()) {
                return Err(bad(cmd, format!("A musician plays into a MIDI or instrument track (`{}` is not one)", t.name)));
            }
        }
        None => {}
    }
    let s = e.session_mut();
    let mut r = parse_route(s, &v, input);
    if let Route::Hardware(name) = &r
        && create_bus
        && !name.to_ascii_lowercase().starts_with("in ")
        && !(input && crate::io::is_remote_input(name))
        && named_path(s, name, input).is_none()
    {
        let fmt = tracks
            .first()
            .and_then(|t| s.track(*t))
            .map_or(ChannelFormat::Stereo, |t| if t.kind == TrackKind::Aux { t.format } else { ChannelFormat::Stereo });
        r = Route::Bus(s.add_bus(&name.clone(), fmt));
    }
    for t in &tracks {
        if let Some(tr) = s.track_mut(*t) {
            if input {
                tr.mixer.input = r.clone();
            } else {
                tr.mixer.output = r.clone();
            }
        }
    }
    Ok(json!({key: r}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_input_routes() {
        let mut e = Engine::default();
        let keys = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let tid = soundcraft_model::TrackId(keys);
        let input = |e: &Engine| e.session().track(tid).unwrap().mixer.input.clone();
        let midi = |device: Option<&str>, channel| Route::Midi(soundcraft_model::MidiIn { device: device.map(str::to_string), channel });
        assert_eq!(input(&e), midi(None, 0), "a new instrument track takes MIDI: All Inputs");
        e.execute("track.input", &json!({"tracks": [keys], "input": "MIDI: Focusrite USB MIDI", "channel": 3})).unwrap();
        assert_eq!(input(&e), midi(Some("Focusrite USB MIDI"), 3));
        e.execute("track.input", &json!({"tracks": [keys], "input": "midi:all", "channel": "omni"})).unwrap();
        assert_eq!(input(&e), midi(None, 0));
        assert!(e.execute("track.input", &json!({"tracks": [keys], "input": "midi:all", "channel": 17})).is_err());
        assert!(e.execute("track.output", &json!({"tracks": [keys], "output": "midi:all"})).is_err());
        let audio = e.execute("track.new", &json!({})).unwrap()["tracks"][0].as_u64().unwrap();
        assert!(e.execute("track.input", &json!({"tracks": [audio], "input": "midi:all"})).is_err());
        // Saved and loaded with the session.
        let text = serde_json::to_string(e.session()).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&text).unwrap();
        assert_eq!(back.track(tid).unwrap().mixer.input, midi(None, 0));
        assert_eq!(soundcraft_model::MidiIn { device: Some("Keys".into()), channel: 2 }.label(), "MIDI: Keys (ch 2)");
    }

    #[test]
    fn track_view_accepts_pitch() {
        let mut e = crate::demo::demo_engine();
        let t = e.session().track_by_name("Kick").map(|t| t.id).unwrap();
        e.execute("track.view", &json!({"track": t.0, "view": "pitch"})).unwrap();
        assert_eq!(e.session().track(t).unwrap().view, "pitch");
    }

    #[test]
    fn comp_from_alternate_playlist() {
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").map(|t| t.id.0).unwrap();
        // Duplicate the playlist, then clear a range on the new active playlist and promote it back from the first.
        e.execute("track.playlist_duplicate", &json!({"track": kick})).unwrap();
        e.execute("edit.clear", &json!({"tracks": [kick], "start": {"seconds": 10.0}, "end": {"seconds": 12.0}})).unwrap();
        let gap = |e: &Engine| {
            e.session().track(soundcraft_model::TrackId(kick)).unwrap().playlist().unwrap().clip_at(e.session().sample_rate.samples(11.0)).is_none()
        };
        assert!(gap(&e));
        e.execute("track.playlist_promote", &json!({"track": kick, "playlist": 0, "start": {"seconds": 10.0}, "end": {"seconds": 12.0}})).unwrap();
        assert!(!gap(&e));
        assert!(e.execute("track.playlist_promote", &json!({"track": kick, "playlist": 1, "start": 0, "end": 10})).is_err());
    }

    #[test]
    fn group_modify_changes_name_membership_and_linking() {
        let mut e = crate::demo::demo_engine();
        let g = e.execute("track.group", &json!({"tracks": ["Kick", "Snare"], "name": "Drums"})).unwrap()["group"].as_u64().unwrap();
        // Every field at once, the way the dialog submits it.
        let r = e.execute("track.group_modify", &json!({"group": g, "name": "Rhythm", "tracks": ["Kick", "Bass"], "edit": false})).unwrap();
        assert_eq!(r["name"], "Rhythm");
        let gr = e.session().groups.iter().find(|x| x.id.0 == g).unwrap().clone();
        assert_eq!(gr.name, "Rhythm");
        assert_eq!(gr.members, vec![e.session().track_by_name("Kick").unwrap().id, e.session().track_by_name("Bass").unwrap().id]);
        assert!(!gr.edit);
        assert!(gr.mix, "an omitted field keeps its value");
        assert_eq!(gr.letter, 'a', "the letter is the group's stable id and never changes");
        // Omitting `tracks` leaves membership alone rather than falling back to the selection.
        e.execute("edit.select", &json!({"tracks": ["Pad"], "exact": true})).unwrap();
        e.execute("track.group_modify", &json!({"group": g, "name": "Rhythm Section"})).unwrap();
        assert_eq!(e.session().groups.iter().find(|x| x.id.0 == g).unwrap().members, gr.members);
        // Resolving the group by its new name keeps working, so names stay unique and non-empty.
        e.execute("track.group", &json!({"tracks": ["Pad"], "name": "Pads"})).unwrap();
        assert!(e.execute("track.group_modify", &json!({"group": "Pads", "name": "Rhythm Section"})).is_err());
        assert!(e.execute("track.group_modify", &json!({"group": g, "name": "  "})).is_err());
        assert!(e.execute("track.group_modify", &json!({"group": g, "tracks": []})).is_err());
        assert!(e.execute("track.group_modify", &json!({"group": g, "tracks": "Kick"})).is_err(), "`tracks` must be an array");
        assert!(e.execute("track.group_modify", &json!({"group": g, "tracks": ["Kick", "Kick"]})).is_ok());
        assert_eq!(e.session().groups.iter().find(|x| x.id.0 == g).unwrap().members.len(), 1, "a repeated track is listed once");
        assert!(e.execute("track.group_modify", &json!({"group": 9999, "name": "x"})).is_err());
        assert_eq!(e.session().groups.iter().find(|x| x.id.0 == g).unwrap().name, "Rhythm Section", "a rejected call changes nothing");
    }

    #[test]
    fn folder_routes_members_through_its_bus() {
        let mut e = crate::demo::demo_engine();
        let r = e.execute("track.move_to_new_folder", &json!({"tracks": ["Kick", "Snare"], "name": "Drums"})).unwrap();
        assert!(r["bus"].is_u64());
        let s = e.session();
        let kick = s.track_by_name("Kick").unwrap();
        let snare = s.track_by_name("Snare").unwrap();
        let folder = s.tracks.iter().find(|track| track.kind == TrackKind::Folder).unwrap();
        assert!(matches!(kick.mixer.output, Route::Bus(_)));
        assert_eq!(folder.mixer.input, kick.mixer.output);
        assert_eq!(folder.mixer.input, snare.mixer.output);
        assert_eq!(s.track_index(folder.id).unwrap() + 1, s.track_index(kick.id).unwrap());
        assert_eq!(s.track_index(kick.id).unwrap() + 1, s.track_index(snare.id).unwrap());
    }

    #[test]
    fn empty_folder_owns_a_unique_bus_and_renames_it() {
        let mut e = crate::demo::demo_engine();
        let first = e.execute("track.new", &json!({"kind": "folder", "name": "Guitars"})).unwrap();
        let second = e.execute("track.new", &json!({"kind": "folder", "name": "Vocals"})).unwrap();
        let first = TrackId(first["tracks"][0].as_u64().unwrap());
        let second = TrackId(second["tracks"][0].as_u64().unwrap());
        let first_bus = match e.session().track(first).unwrap().mixer.input {
            Route::Bus(bus) => bus,
            _ => panic!("folder did not receive a bus"),
        };
        let second_bus = match e.session().track(second).unwrap().mixer.input {
            Route::Bus(bus) => bus,
            _ => panic!("folder did not receive a bus"),
        };
        assert_ne!(first_bus, second_bus);
        e.session_mut().add_bus("Percussion Bus", ChannelFormat::Stereo);
        e.execute("track.rename", &json!({"track": first.0, "name": "Percussion"})).unwrap();
        let renamed = e.session().busses.iter().find(|bus| bus.id == first_bus).unwrap();
        assert_eq!(renamed.name, "Percussion Bus 2");
    }

    #[test]
    fn tracks_can_move_between_nested_folders_and_leave() {
        let mut e = crate::demo::demo_engine();
        let outer = e.execute("track.new", &json!({"kind": "folder", "name": "Band"})).unwrap();
        let inner = e.execute("track.new", &json!({"kind": "folder", "name": "Drums"})).unwrap();
        let outer = TrackId(outer["tracks"][0].as_u64().unwrap());
        let inner = TrackId(inner["tracks"][0].as_u64().unwrap());

        e.execute("track.move_to_folder", &json!({"tracks": ["Kick"], "folder": inner.0})).unwrap();
        e.execute("track.move_to_folder", &json!({"tracks": [inner.0], "folder": outer.0})).unwrap();
        let inner_bus = match e.session().track(inner).unwrap().mixer.input {
            Route::Bus(bus) => bus,
            _ => panic!("folder did not receive a bus"),
        };
        let outer_bus = match e.session().track(outer).unwrap().mixer.input {
            Route::Bus(bus) => bus,
            _ => panic!("folder did not receive a bus"),
        };
        assert!(e.execute("track.move_to_folder", &json!({"tracks": [outer.0], "folder": inner.0})).is_err());

        let kick = e.session().track_by_name("Kick").unwrap();
        assert_eq!(kick.folder, Some(inner));
        assert_eq!(kick.mixer.output, e.session().track(inner).unwrap().mixer.input);
        assert_eq!(e.session().track_index(outer).unwrap() + 1, e.session().track_index(inner).unwrap());
        assert_eq!(e.session().track_index(inner).unwrap() + 1, e.session().track_index(kick.id).unwrap());

        e.execute("track.remove_from_folder", &json!({"tracks": ["Kick"]})).unwrap();
        let kick = e.session().track_by_name("Kick").unwrap();
        assert_eq!(kick.folder, Some(outer));
        assert_eq!(kick.mixer.output, e.session().track(outer).unwrap().mixer.input);

        e.execute("track.move_to_folder", &json!({"tracks": ["Kick"], "folder": inner.0})).unwrap();
        e.execute("track.remove_from_folder", &json!({"tracks": [inner.0]})).unwrap();
        let inner_track = e.session().track(inner).unwrap();
        let kick = e.session().track_by_name("Kick").unwrap();
        assert_eq!(inner_track.folder, None);
        assert_eq!(inner_track.mixer.output, Route::Main);
        assert_eq!(kick.folder, Some(inner));
        assert_eq!(e.session().track_index(inner).unwrap() + 1, e.session().track_index(kick.id).unwrap());

        e.execute("track.delete", &json!({"tracks": [inner.0]})).unwrap();
        let kick = e.session().track_by_name("Kick").unwrap();
        assert_eq!(kick.folder, None);
        assert_eq!(kick.mixer.output, Route::Main);
        assert!(e.session().bus(inner_bus).is_none(), "the deleted folder's owned bus must be removed");
        assert!(e.session().bus(outer_bus).is_some(), "the surviving parent folder's bus must remain");
    }

    #[test]
    fn heights_take_points_and_step_through_the_presets() {
        let mut e = crate::demo::demo_engine();
        let height = |e: &Engine, name: &str| e.session().track_by_name(name).map(|t| t.height);
        e.execute("track.height", &json!({"tracks": ["Kick"], "height": 57})).unwrap();
        assert_eq!(height(&e, "Kick"), Some(TrackHeight::Custom(57)));
        e.execute("track.height_up", &json!({"tracks": ["Kick", "Snare"]})).unwrap();
        assert_eq!((height(&e, "Kick"), height(&e, "Snare")), (Some(TrackHeight::Medium), Some(TrackHeight::Large)));
        e.execute("track.height", &json!({"tracks": ["Kick"], "height": "extreme"})).unwrap();
        let steps = e.undo_history().len();
        assert_eq!(e.execute("track.height_up", &json!({"tracks": ["Kick"]})).unwrap()["changed"], 0);
        assert_eq!(e.undo_history().len(), steps);
        e.execute("track.height_down", &json!({"tracks": ["Kick"]})).unwrap();
        assert_eq!(height(&e, "Kick"), Some(TrackHeight::Jumbo));
    }

    #[test]
    fn vca_null_clears_the_assignment() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "Member"})).unwrap();
        e.execute("track.new", &json!({"kind": "vca", "name": "Master"})).unwrap();
        let (member, master) = (e.session().tracks[0].id, e.session().tracks[1].id);
        let vca = |e: &Engine| e.session().track(member).unwrap().mixer.vca;
        e.execute("track.vca", &json!({"tracks": [member.0], "vca": master.0})).unwrap();
        assert_eq!(vca(&e), Some(master.0));
        // #237: null is documented as "no VCA"; anything else that is not a VCA is still an error.
        for invalid in [json!(true), json!(999), json!("Missing"), json!(member.0)] {
            assert!(e.execute("track.vca", &json!({"tracks": [member.0], "vca": invalid})).is_err());
            assert_eq!(vca(&e), Some(master.0));
        }
        e.execute("track.vca", &json!({"tracks": [member.0], "vca": null})).unwrap();
        assert_eq!(vca(&e), None);
        assert!(e.undo());
        assert_eq!(vca(&e), Some(master.0));
    }

    #[test]
    fn automation_lanes_show_hide_order_and_follow_their_module() {
        let mut e = crate::demo::demo_engine();
        e.execute("mix.module_add", &json!({"track": "Kick", "plugin": "fs_cleanq"})).unwrap();
        let t = e.session().track_by_name("Kick").unwrap();
        let (t, m) = (t.id, t.mixer.inserts.iter().find(|i| i.plugin == "fs_cleanq").unwrap().id);
        let lanes = |e: &Engine| e.session().track(t).unwrap().auto_lanes.clone();
        assert_eq!(e.execute("track.auto_lane", &json!({"track": t.0, "param": "pan"})).unwrap(), json!({"shown": true}));
        e.execute("track.auto_lane", &json!({"track": t.0, "param": "VOLUME", "show": true})).unwrap();
        e.execute("track.auto_lane", &json!({"track": t.0, "param": format!("plugin:m{}:outgain", m.0), "after": "pan"})).unwrap();
        assert_eq!(lanes(&e), ["pan", &format!("plugin:m{}:outGain", m.0), "volume"]);
        // Toggling hides; showing again is idempotent.
        e.execute("track.auto_lane", &json!({"track": t.0, "param": "pan"})).unwrap();
        e.execute("track.auto_lane", &json!({"track": t.0, "param": "volume", "show": true})).unwrap();
        assert_eq!(lanes(&e), [format!("plugin:m{}:outGain", m.0), "volume".into()]);
        assert!(e.execute("track.auto_lane", &json!({"track": t.0, "param": "nonsense"})).is_err());
        assert!(e.execute("track.auto_lane", &json!({"track": t.0, "param": "plugin:m999:x"})).is_err());
        // Removing the module drops its lane.
        e.execute("mix.module_remove", &json!({"track": t.0, "module": m.0})).unwrap();
        assert_eq!(lanes(&e), ["volume"]);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(lanes(&e).len(), 2);
    }

    #[test]
    fn show_lanes_with_data_adds_every_automated_parameter_once() {
        let mut e = crate::demo::demo_engine();
        let t = e.session().track_by_name("Kick").unwrap().id;
        e.execute("track.auto_lanes", &json!({"track": t.0, "show": "data"})).unwrap();
        assert_eq!(e.session().track(t).unwrap().auto_lanes, ["volume"], "nothing automated: volume");
        e.execute("automation.set_point", &json!({"track": t.0, "param": "mute", "at": 0, "value": 1.0})).unwrap();
        e.execute("automation.set_point", &json!({"track": t.0, "param": "send_b_level", "at": 0, "value": -3.0})).unwrap();
        e.execute("track.auto_lanes", &json!({"track": t.0, "show": "data"})).unwrap();
        assert_eq!(e.session().track(t).unwrap().auto_lanes, ["volume", "mute", "send_b_level"]);
        e.execute("track.auto_lanes", &json!({"track": t.0, "show": "none"})).unwrap();
        assert!(e.session().track(t).unwrap().auto_lanes.is_empty());
        assert!(e.execute("track.auto_lanes", &json!({"track": t.0, "show": "some"})).is_err());
        // Saved and loaded with the session.
        let json = serde_json::to_string(e.session()).unwrap();
        assert!(!json.contains("auto_lanes"), "empty lists are not written");
        e.execute("track.auto_lane", &json!({"track": t.0, "param": "mute"})).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&serde_json::to_string(e.session()).unwrap()).unwrap();
        assert_eq!(back.track(t).unwrap().auto_lanes, ["mute"]);
    }

    #[test]
    fn sends_trim_and_the_second_panner_are_lanes_and_views() {
        let mut e = crate::demo::demo_engine();
        let t = e.session().track_by_name("Kick").unwrap().id;
        for p in ["send_a_level", "send_b_pan", "send_j_mute", "trim", "pan2"] {
            e.execute("track.auto_lane", &json!({"track": t.0, "param": p, "show": true})).unwrap();
            e.execute("track.view", &json!({"track": t.0, "view": p})).unwrap();
            e.execute("automation.set_point", &json!({"track": t.0, "param": p, "at": 0, "value": 0.0})).unwrap();
        }
        assert_eq!(e.session().track(t).unwrap().auto_lanes, ["send_a_level", "send_b_pan", "send_j_mute", "trim", "pan2"]);
        assert!(e.execute("track.auto_lane", &json!({"track": t.0, "param": "send_k_level"})).is_err());
    }
}
