//! The reader on the two clean-room sessions of ptx-session-kit (`fixtures/README.md`), checked
//! against what the kit's build specification for them says (`CR_min.json`, not committed).

use soundcraft_ptx::decode::tracks::TrackPlaylists;
use soundcraft_ptx::{PtxDocument, decode_bytes};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

fn doc(name: &str) -> PtxDocument {
    decode_bytes(name, &fixture(name)).unwrap()
}

#[test]
fn cr_min_tracks_tempo_markers_clips_and_notes() {
    let d = doc("CR_min.ptx");
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(d.version.as_deref(), Some("2026.11.0"));
    assert_eq!((d.format.sample_rate, d.format.bit_depth), (Some(48_000), Some(24)));
    assert_eq!(d.timeline.session_start.as_deref(), Some("01:00:00:00"));
    assert_eq!(d.timeline.timecode_rate.as_deref(), Some("25"));
    let tracks: Vec<(&str, u16, bool)> = d.tracks.iter().map(|t| (t.name.as_str(), t.kind, t.stereo)).collect();
    assert_eq!(tracks, vec![("Au_Lead", 0, false), ("In_Keys", 7, true), ("Md_Bass", 1, false), ("Master", 5, true)]);
    assert_eq!(d.tracks[0].colour, Some(29));
    assert_eq!(d.tracks[0].comment.as_deref(), Some("Clean-room mono audio track"));
    // 120 BPM 4/4 from the start; C major, then A minor at bar 2 (3 840 000 ticks).
    assert_eq!(d.tempo_used.iter().map(|t| (t.ticks, t.bpm)).collect::<Vec<_>>(), vec![(0, 120.0)]);
    assert_eq!(d.meter_used.iter().map(|m| (m.numerator, m.denominator)).collect::<Vec<_>>(), vec![(4, 4)]);
    assert_eq!(d.key_signatures.iter().map(|k| (k.ticks, k.key.as_str())).collect::<Vec<_>>(), vec![(0, "C"), (3_840_000, "Am")]);
    // Markers: Intro, and the Chorus selection over bar 2.
    let m: Vec<(&str, u64, u64, Option<&str>)> = d.markers.iter().map(|m| (m.name.as_str(), m.ticks, m.end, m.kind.as_str())).collect();
    assert_eq!(m, vec![("Intro", 0, 0, Some("marker")), ("Chorus", 3_840_000, 7_680_000, Some("selection"))]);
    // The audio clip at 0, 192 000 samples, with a 4 800 fade in and a 9 600 fade out.
    let lead = d.regions.iter().find(|r| r.name == "lead_m").unwrap();
    assert_eq!((lead.start, lead.length), (0, 192_000));
    let fades: Vec<(u64, Option<&str>)> = d.fades.iter().map(|f| (f.length, f.fade_type.as_str())).collect();
    assert_eq!(fades, vec![(4_800, Some("fade_in")), (9_600, Some("fade_out"))]);
    // MIDI: the four chords (13 notes) and the four bass notes.
    assert_eq!(d.midi_lists.iter().map(|c| c.notes.len()).collect::<Vec<_>>(), vec![13, 4]);
    assert_eq!(d.midi_lists[1].notes.iter().map(|n| n[1]).collect::<Vec<_>>(), vec![36, 33, 29, 31]);
    // Mixer: the lead's fader lane in tenths of a dB, its pan.
    let st = d.strips.iter().find(|s| s.track == Some(0)).unwrap();
    assert_eq!(st.volume, vec![(0, 0), (96_000, -30), (192_000, 0)]);
    assert_eq!(st.pan.first(), Some(&(0, -40)));
    // The stock plug-ins named from the session's plug-in table.
    let plugins: Vec<&str> = d.tracks.iter().flat_map(|t| t.inserts.iter().flatten()).filter_map(|i| i.plugin.as_deref()).collect();
    assert_eq!(plugins, vec!["EQ3 7-Band", "Dyn3 Compressor/Limiter", "Trim"]);
}

#[test]
fn cr_max3_thirty_tracks_folders_groups_and_clip_positions() {
    let d = doc("CR_max3.ptx");
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    assert_eq!(d.tracks.len(), 30);
    assert_eq!(d.tracks.iter().filter(|t| t.kind == 0).count(), 8, "audio tracks");
    assert_eq!(d.tracks.iter().filter(|t| t.kind == 7).count(), 8, "instrument tracks");
    assert_eq!(d.tracks.iter().filter(|t| t.kind == 1).count(), 8, "MIDI tracks");
    assert_eq!(d.tracks[0].track_type, "RoutingFolder");
    let folder = |n: &str| d.tracks.iter().find(|t| t.name == n).and_then(|t| t.folder.clone());
    assert_eq!(folder("Au_Drums_St").as_deref(), Some("Fold_Rhythm"));
    assert_eq!(folder("In_Piano_St").as_deref(), Some("Fold_Strings"));
    let groups: Vec<(&str, u8, usize)> = d.group_details.iter().map(|g| (g.name.as_str(), g.group_type, g.members.len())).collect();
    assert_eq!(groups, vec![("<ALL>", 2, 30), ("Grp_Rhythm", 3, 3), ("Grp_Synths", 1, 3), ("Grp_AuxMix", 2, 3)]);
    assert_eq!(d.tempo_used.iter().map(|t| (t.ticks, t.bpm)).collect::<Vec<_>>(), vec![(0, 120.0), (3_840_000, 132.0), (7_680_000, 112.0)]);
    assert_eq!(d.markers.len(), 4);
    assert_eq!(d.markers[3].samples, Some(96_000), "the absolute marker is in samples");
    // The bass clip: placed at 10 000 samples with a 10 000-sample sync point, so it starts at 0.
    let bass = d.audio_channels.get("Au_Bass_M").and_then(|chs| chs.first()).and_then(|c| c.as_array()).and_then(|a| a.first()).unwrap();
    assert_eq!((bass["samples"].as_i64(), bass["sync_offset"].as_u64(), bass["clip_start_samples"].as_i64()), (Some(10_000), Some(10_000), Some(0)));
    // The guitar clip loops four times; the drums sit in ticks.
    let gtr = d.audio_placements.get("Au_Gtr_St").unwrap();
    assert!(gtr.iter().any(|p| p["loop"]["count"].as_u64() == Some(4)));
    // Alternate playlists of the bass and the vocal.
    match d.track_playlists.get("Au_Bass_M") {
        Some(TrackPlaylists::Audio { alternates, .. }) => {
            assert_eq!(alternates.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Au_Bass_M.alt"])
        }
        other => panic!("{other:?}"),
    }
    // The CC tracks between them hold CC 1 to 119.
    let ccs: usize = d.midi_lists.iter().map(|c| c.controllers.iter().filter(|k| k.kind.as_str() == Some("controller")).count()).sum();
    assert!(ccs >= 119, "{ccs}");
    assert_eq!(d.clip_gain_lanes.first().map(|l| l.points.len()), Some(4));
}
