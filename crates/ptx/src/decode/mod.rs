// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/decode/mod.rs`. MIT licence, Copyright (c) 2026 John Forward
// (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: `decode_file` removed (the engine reads files through its checked file
// access); `decode_bytes` refuses files over `MAX_PTX_BYTES`; a preset name is cut at a character
// boundary (slicing a name that ends in a multi-byte character panicked); clip start arithmetic
// saturates; no indexing.
//! Clean-room `.ptx` session decoder.

pub mod clips;
pub mod midi;
pub mod mixer;
pub mod plugins;
pub mod timeline;
pub mod tracks;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::PtxError;
use crate::scramble::descramble;
use crate::tree::{Block, parse_blocks};

use clips::{
    AudioPlacement, ClipGainLane, ClipGroupRegion, FadeObject, MidiPlacement, PlaylistMeta, Region, parse_audio_file_exts, parse_audio_file_stems,
    parse_audio_placements, parse_audio_playlists, parse_clip_gain_lanes, parse_clip_gains, parse_clip_group_playlists, parse_clip_group_regions,
    parse_fades, parse_midi_placements, parse_playlists, parse_region_gain_links, parse_region_lists, parse_regions, parse_umids,
};
use midi::{MidiChunk, parse_midi_lists};
use mixer::{Strip, parse_strips, subtree};
use plugins::{PluginTableEntry, SettingsFile, parse_inserts, parse_plugin_table, parse_settings_files};
use timeline::{
    KeySignature, Marker, MeterEvent, SessionFormat, SessionSettings, TempoEvent, TimelineInfo, parse_key_signatures, parse_markers, parse_meter,
    parse_session_format, parse_session_settings, parse_session_version, parse_tempo, parse_timeline, parse_write_enables, tempo_blocks,
    version_tuple,
};
use tracks::{
    GroupDetail, Track, TrackPlaylists, parse_automation_mode, parse_aux_input_path, parse_elastic_mode, parse_elastic_params, parse_folder_map,
    parse_group_details, parse_groups, parse_mpe_mode, parse_solo, parse_track_colours, parse_track_comment, parse_track_input,
    parse_track_playlists, parse_track_timebase, parse_tracks, parse_user_delay,
};

/// Canonical decoded Pro Tools `.ptx` session document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PtxDocument {
    pub file: String,
    pub tracks: Vec<Track>,
    pub markers: Vec<Marker>,
    pub tempo: Vec<TempoEvent>,
    pub tempo_copies: usize,
    pub tempo_second: Vec<TempoEvent>,
    pub tempo_used: Vec<TempoEvent>,
    pub meter: Vec<MeterEvent>,
    pub meter_second: Vec<MeterEvent>,
    pub meter_used: Vec<MeterEvent>,
    pub regions: Vec<Region>,
    pub placements: Vec<Vec<MidiPlacement>>,
    pub strips: Vec<Strip>,
    pub umids: BTreeMap<String, String>,
    #[serde(skip)]
    pub audio_file_stems: Vec<String>,
    #[serde(skip)]
    pub audio_file_exts: BTreeMap<String, String>,
    pub version: Option<String>,
    pub format: SessionFormat,
    pub timeline: TimelineInfo,
    pub session_settings: SessionSettings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<PluginTableEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_files: Option<Vec<SettingsFile>>,
    pub write_enables: Option<Vec<u8>>,
    pub playlists: Vec<PlaylistMeta>,
    pub clip_gains: Vec<f64>,
    pub clip_gain_lanes: Vec<ClipGainLane>,
    pub audio_placements: BTreeMap<String, Vec<serde_json::Value>>,
    pub fades: Vec<FadeObject>,
    pub audio_playlists: Vec<(String, Vec<AudioPlacement>)>,
    pub audio_channels: BTreeMap<String, Vec<serde_json::Value>>,
    pub clip_groups: BTreeMap<String, Vec<serde_json::Value>>,
    #[serde(skip)]
    pub clip_group_playlists: Vec<Vec<AudioPlacement>>,
    pub clip_group_regions: Vec<ClipGroupRegion>,
    pub midi_lists: Vec<MidiChunk>,
    pub groups: Vec<String>,
    pub group_details: Vec<GroupDetail>,
    pub track_playlists: BTreeMap<String, TrackPlaylists>,
    pub key_signatures: Vec<KeySignature>,
    pub warnings: Vec<String>,
}

/// Descramble raw `.ptx` bytes, parse blocks, and decode into `PtxDocument`.
///
/// # Errors
/// Returns scramble or container parse errors on malformed inputs.
pub fn decode_bytes(file_name: &str, raw: &[u8]) -> Result<PtxDocument, PtxError> {
    if raw.len() > crate::MAX_PTX_BYTES {
        return Err(PtxError::TooLarge(raw.len()));
    }
    let plain = descramble(raw)?;
    let tree = parse_blocks(&plain)?;
    Ok(decode_plain(file_name, &plain, &tree))
}

/// Decode an already-descrambled `.ptx` byte buffer and parsed block tree.
#[must_use]
pub fn decode_plain(file_name: &str, d: &[u8], tree: &[Block]) -> PtxDocument {
    let mut warn = Vec::new();
    let mut tracks_list = parse_tracks(d, tree, &mut warn);
    let markers_list = parse_markers(d, tree, &mut warn);
    let tempo_first = parse_tempo(d, tree, &mut warn, 0);
    let tempo_copies = tempo_blocks(tree).len();
    let meter_first = parse_meter(d, tree, &mut warn, 0);
    let (mut regions_list, region_positions) = parse_regions(d, tree, &mut warn);
    let placements_list = parse_midi_placements(d, tree);
    let strips_list = parse_strips(d, tree);
    let umids_map = parse_umids(d, tree);
    let version = parse_session_version(d, tree);
    let format = parse_session_format(d, tree);
    let timeline_info = parse_timeline(d, tree);
    let session_settings = parse_session_settings(d, tree);

    let fmap = parse_folder_map(d, tree, &tracks_list);
    for t in &mut tracks_list {
        t.folder = fmap.get(&t.name).cloned();
    }

    let top = Block::find(tree, 0x2624);
    let mut plugins_opt = None;
    let mut settings_opt = None;
    let carry_indices: Vec<usize> = tracks_list.iter().enumerate().filter_map(|(idx, t)| matches!(t.kind, 0 | 2 | 5 | 7).then_some(idx)).collect();

    if let Some(top_blk) = top
        && top_blk.kids.len() == tracks_list.len()
        && strips_list.iter().all(|st| st.track.is_some())
    {
        for st in &strips_list {
            if let Some(tidx) = st.track
                && let Some(t) = tracks_list.get_mut(tidx)
            {
                t.output = st.output.clone();
                t.stem_index = st.stem_index;
                t.active = st.active;
                if let Some(re) = st.record_enable {
                    t.record_enable = Some(re);
                }
            }
        }
        let ptab = parse_plugin_table(d, tree);
        let sfiles = parse_settings_files(d, tree);
        let vt = version_tuple(version.as_deref());
        let is_2026_11 = vt.first().copied().unwrap_or(0) > 2026 || (vt.first().copied() == Some(2026) && vt.get(1).copied().unwrap_or(0) >= 11);
        let folder_names: std::collections::BTreeSet<String> = fmap.values().cloned().collect();

        for (t, ch) in tracks_list.iter_mut().zip(top_blk.kids.iter()) {
            if ch.content_type == 0x261E {
                let mut kc = Vec::new();
                subtree(ch, &mut kc);
                let c2038 = kc.iter().filter(|x| x.content_type == 0x2038).count();
                let c203b = kc.iter().filter(|x| x.content_type == 0x203B).count();
                if c2038 > c203b || folder_names.contains(&t.name) {
                    t.track_type = "RoutingFolder".to_owned();
                }
            }
            t.automation_mode = parse_automation_mode(d, ch);
            t.user_delay = parse_user_delay(d, ch);
            let (cm, hc) = parse_track_comment(d, ch);
            t.comment = cm;
            t.height_code = hc;
            t.solo = parse_solo(d, ch);
            t.hidden = Some(t.number == 0);
            if let Some(el_m) = parse_elastic_mode(d, ch) {
                t.elastic_mode = Some(el_m);
                let el_p = parse_elastic_params(d, ch);
                if !el_p.is_empty() {
                    t.elastic_params = Some(el_p);
                }
            }
            if let Some(tb_m) = parse_track_timebase(d, ch) {
                t.timebase = Some(tb_m);
            }
            if matches!(ch.content_type, 0x2620 | 0x2621) && is_2026_11 {
                t.mpe = parse_mpe_mode(d, ch);
            }
            let mut ins = parse_inserts(d, ch);
            if !ins.is_empty() {
                for e in &mut ins {
                    let p_entry = e.table_index.and_then(|idx| ptab.get(usize::from(idx)));
                    e.plugin = p_entry.and_then(|p| p.name.clone());
                    e.vendor_id = p_entry.map_or_else(String::new, |p| p.vendor_id.clone());
                    e.stock = p_entry.is_some_and(|p| p.vendor_id.starts_with("com.avid.") || p.vendor_id.starts_with("com.digidesign."));
                    let sf_entry = e.settings_index.and_then(|idx| sfiles.get(usize::from(idx)));
                    e.settings_file = sf_entry.map(|s| s.file.clone());
                    e.preset = sf_entry
                        .and_then(|s| (!s.factory_default && s.file.len() >= 4).then(|| s.file.get(..s.file.len() - 4).map(str::to_owned)).flatten());
                }
                t.inserts = Some(ins);
            }
            if ch.content_type == 0x261E {
                t.input_path = parse_aux_input_path(d, ch);
            }
            let (in_nm, in_ty) = parse_track_input(d, ch);
            if in_nm.is_some() || in_ty.is_some() {
                t.input = in_nm;
                t.input_type = in_ty;
            }
        }
        plugins_opt = Some(ptab);
        settings_opt = Some(sfiles);
    } else if let Some(top_blk) = top {
        warn.push(format!("{} channel blocks for {} tracks: outputs not assigned", top_blk.kids.len(), tracks_list.len()));
    } else if carry_indices.len() == strips_list.len() {
        for (tidx, st) in carry_indices.into_iter().zip(strips_list.iter()) {
            if let Some(t) = tracks_list.get_mut(tidx) {
                t.output = st.output.clone();
                t.stem_index = st.stem_index;
            }
        }
    } else if !strips_list.is_empty() {
        warn.push(format!("{} strips for {} audio-carrying tracks: outputs not assigned", strips_list.len(), carry_indices.len()));
    }

    let write_enables = parse_write_enables(d, tree);
    let playlists_list = parse_playlists(d, tree);
    let clip_gains = parse_clip_gains(d, tree);
    let clip_gain_lanes = parse_clip_gain_lanes(d, tree);
    let links = parse_region_gain_links(d, tree);
    for (r, pos) in regions_list.iter_mut().zip(region_positions) {
        if let Some(&link_opt) = links.get(&pos) {
            r.clip_gain_index = Some(link_opt);
            r.clip_gain = link_opt.and_then(|idx| clip_gains.get(idx).copied());
        }
    }

    let mut audio_placements_map = parse_audio_placements(d, tree, false, false);
    let fades_list = parse_fades(d, tree);
    let audio_playlists_list = parse_audio_playlists(d, tree);

    let mut warn_unused = Vec::new();
    let tempo_second = parse_tempo(d, tree, &mut warn_unused, 1);
    let meter_second = parse_meter(d, tree, &mut warn_unused, 1);
    let tempo_used = if tempo_second.is_empty() { tempo_first.clone() } else { tempo_second.clone() };
    let meter_used = if meter_second.is_empty() { meter_first.clone() } else { meter_second.clone() };
    if !tempo_second.is_empty() && tempo_second != tempo_first {
        warn.push("the two tempo copies differ".to_owned());
    }
    if !meter_second.is_empty() && meter_second != meter_first {
        warn.push("the two meter copies differ".to_owned());
    }

    for (r, li) in regions_list.iter_mut().zip(parse_region_lists(d, tree)) {
        r.list = Some(li);
    }

    let aud: Vec<(String, u64, Option<u64>)> =
        regions_list.iter().filter(|r| r.list.is_none()).map(|r| (r.name.clone(), r.length, r.sync_offset)).collect();
    let mut audio_channels_map = parse_audio_placements(d, tree, true, false);
    let mut clip_groups_map = parse_audio_placements(d, tree, true, true);
    let mut clip_group_pls = parse_clip_group_playlists(d, tree);
    let cgrs = parse_clip_group_regions(d, tree);
    let tempo_refs: Vec<(u64, f64)> = tempo_used.iter().map(|t| (t.ticks, t.bpm)).collect();
    let sr_f = f64::from(format.sample_rate.unwrap_or(48_000).max(1));

    let enrich = |item: &mut serde_json::Value| {
        let Some(obj) = item.as_object_mut() else {
            return;
        };
        let kind = obj.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let idx = obj.get("index").and_then(|v| v.as_u64()).and_then(|u| usize::try_from(u).ok());
        if kind == "clip"
            && let Some(i) = idx
            && let Some((nm, len, sync_off)) = aud.get(i)
        {
            obj.insert("clip_name".to_owned(), serde_json::Value::from(nm.clone()));
            obj.insert("clip_length".to_owned(), serde_json::Value::from(*len));
            if let Some(so) = sync_off {
                obj.insert("sync_offset".to_owned(), serde_json::Value::from(*so));
                if let (Some(samp), Ok(so_i)) = (obj.get("samples").and_then(serde_json::Value::as_i64), i64::try_from(*so)) {
                    obj.insert("clip_start_samples".to_owned(), serde_json::Value::from(samp.saturating_sub(so_i)));
                } else if let (Some(tk), Ok(so_i)) = (obj.get("ticks").and_then(serde_json::Value::as_u64), i64::try_from(*so))
                    && !tempo_refs.is_empty()
                {
                    let sec = crate::primitives::ticks_to_seconds(tk as f64, &tempo_refs);
                    let start_smp = ((sec * sr_f).round() as i64).saturating_sub(so_i);
                    obj.insert("clip_start_samples".to_owned(), serde_json::Value::from(start_smp));
                }
            }
        } else if kind == "group"
            && let Some(i) = idx
            && let Some(g) = cgrs.get(i)
        {
            obj.insert("clip_name".to_owned(), serde_json::Value::from(g.name.clone()));
            obj.insert("clip_length".to_owned(), serde_json::Value::from(g.length));
            if let Some(gid) = g.group_id {
                obj.insert("group_id".to_owned(), serde_json::Value::from(gid));
            }
            if let Some(mpi) = g.member_playlist_index {
                obj.insert("member_playlist_index".to_owned(), serde_json::Value::from(mpi));
            }
        }
    };

    for lst in audio_placements_map.values_mut() {
        for item in lst {
            enrich(item);
        }
    }
    for channels_vec in audio_channels_map.values_mut() {
        for ch_val in channels_vec {
            if let Some(arr) = ch_val.as_array_mut() {
                for item in arr {
                    enrich(item);
                }
            }
        }
    }
    for groups_vec in clip_groups_map.values_mut() {
        for ch_val in groups_vec {
            if let Some(arr) = ch_val.as_array_mut() {
                for item in arr {
                    enrich(item);
                }
            }
        }
    }
    for pl in &mut clip_group_pls {
        for ent in pl.iter_mut() {
            if ent.kind.as_str() == Some("clip")
                && let Some((nm, len, sync_off)) = aud.get(usize::from(ent.index))
            {
                ent.clip_name = Some(nm.clone());
                ent.clip_length = Some(*len);
                if let Some(so) = sync_off {
                    ent.sync_offset = Some(*so);
                    if let (Some(samp), Ok(so_i)) = (ent.samples, i64::try_from(*so)) {
                        ent.clip_start_samples = Some(samp.saturating_sub(so_i));
                    } else if let (Some(tk), Ok(so_i)) = (ent.ticks, i64::try_from(*so))
                        && !tempo_refs.is_empty()
                    {
                        let sec = crate::primitives::ticks_to_seconds(tk as f64, &tempo_refs);
                        ent.clip_start_samples = Some(((sec * sr_f).round() as i64).saturating_sub(so_i));
                    }
                }
            }
        }
    }

    let cols = parse_track_colours(d, tree);
    for t in &mut tracks_list {
        if let Some(c) = cols.get(&t.name) {
            t.colour = *c;
        }
    }

    let midi_lists_vec = parse_midi_lists(d, tree, &mut warn);
    let groups_vec = parse_groups(d, tree);
    let group_details_vec = parse_group_details(d, tree, &tracks_list);
    let track_playlists_map = parse_track_playlists(d, tree, &tracks_list);
    let key_signatures_vec = parse_key_signatures(d, tree);
    let audio_file_stems = parse_audio_file_stems(d, tree);
    let audio_file_exts = parse_audio_file_exts(d, tree);

    PtxDocument {
        file: file_name.to_owned(),
        tracks: tracks_list,
        markers: markers_list,
        tempo: tempo_first,
        tempo_copies,
        tempo_second,
        tempo_used,
        meter: meter_first,
        meter_second,
        meter_used,
        regions: regions_list,
        placements: placements_list,
        strips: strips_list,
        umids: umids_map,
        audio_file_stems,
        audio_file_exts,
        version,
        format,
        timeline: timeline_info,
        session_settings,
        plugins: plugins_opt,
        settings_files: settings_opt,
        write_enables,
        playlists: playlists_list,
        clip_gains,
        clip_gain_lanes,
        audio_placements: audio_placements_map,
        fades: fades_list,
        audio_playlists: audio_playlists_list,
        audio_channels: audio_channels_map,
        clip_groups: clip_groups_map,
        clip_group_playlists: clip_group_pls,
        clip_group_regions: cgrs,
        midi_lists: midi_lists_vec,
        groups: groups_vec,
        group_details: group_details_vec,
        track_playlists: track_playlists_map,
        key_signatures: key_signatures_vec,
        warnings: warn,
    }
}
