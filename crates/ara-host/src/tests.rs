//! The host against the test plug-in (`test_plugin`), in process through the C ABI.

use crate::host::testing;
use crate::sys::*;
use crate::test_plugin::{self, Behaviour, checksum, factory_ptr, report};
use crate::*;
use std::sync::Arc;

fn tone(channels: usize, frames: usize, seed: f32) -> Arc<MemorySource> {
    let chans = (0..channels).map(|c| (0..frames).map(|i| (i as f32 * 0.013 + seed + c as f32).sin() * 0.7).collect()).collect();
    Arc::new(MemorySource { channels: chans, rate: 48_000.0 })
}

fn as_f64(src: &MemorySource) -> Vec<Vec<f64>> {
    src.channels.iter().map(|c| c.iter().map(|x| f64::from(*x)).collect()).collect()
}

fn seq(key: u64, name: &str) -> SequenceDesc {
    SequenceDesc { key, name: name.into(), order: key as i32, color: Some([0.2, 0.4, 0.6]) }
}

fn source(key: u64, audio: Arc<MemorySource>) -> SourceDesc {
    SourceDesc { key, name: format!("file {key}"), persistent_id: format!("src-{key}"), audio }
}

fn region(key: u64, source: u64, sequence: u64, at: f64) -> RegionDesc {
    RegionDesc {
        key,
        source,
        sequence,
        name: format!("clip {key}"),
        persistent_id: format!("clip-{key}"),
        start_in_source: 0.01,
        duration: 0.5,
        source_duration: 0.5,
        transformation: 0,
        origin: None,
        start_in_song: at,
        color: None,
    }
}

fn graph(name: &str, a: &Arc<MemorySource>, b: &Arc<MemorySource>) -> Graph {
    Graph {
        name: name.into(),
        sequences: vec![seq(1, "Vox"), seq(2, "Gtr")],
        sources: vec![source(10, a.clone()), source(11, b.clone())],
        regions: vec![region(100, 10, 1, 0.0), region(101, 10, 1, 2.0), region(102, 11, 2, 1.0)],
        tempo: Vec::new(),
        bars: Vec::new(),
    }
}

fn no_violations(name: &str) -> test_plugin::Report {
    let r = report(name).expect("the plug-in saw the document");
    assert!(r.violations.is_empty(), "{name}: {:?}", r.violations);
    r
}

#[test]
fn the_factory_is_read_and_checked() {
    let info = read_factory(factory_ptr(Behaviour::Good)).unwrap();
    assert_eq!(info.name, "FreeDAWm Test ARA");
    assert_eq!(info.manufacturer, "FreeDAWm tests");
    assert_eq!(info.version, "1.0.0");
    assert_eq!(info.factory_id, "org.freedawm.test.ara");
    assert_eq!((info.lowest_generation, info.highest_generation, info.generation), (4, 6, Some(6)));
    assert_eq!(info.content_names(), vec!["notes"]);
    assert_eq!(info.compatible_archive_ids, vec!["org.freedawm.test.ara.archive.1".to_string()]);
    assert!(!info.audio_file_chunks);
}

#[test]
fn generations_are_chosen_from_what_both_sides_support() {
    assert_eq!(choose_generation(4, 6), Some(6));
    assert_eq!(choose_generation(4, 5), Some(4), "the 2.x development draft is never chosen");
    assert_eq!(choose_generation(3, 4), Some(4));
    assert_eq!(choose_generation(6, 9), Some(6));
    assert_eq!(choose_generation(1, 3), None, "ARA 1 and the 2.0 draft");
    assert_eq!(choose_generation(5, 5), None);
    assert_eq!(choose_generation(6, 4), None, "a range upside down");
}

#[test]
fn unusable_factories_are_refused_without_a_document() {
    assert!(matches!(Document::new(factory_ptr(Behaviour::Ara1), "ara1"), Err(AraError::Generation(1, 2))));
    assert!(matches!(read_factory(factory_ptr(Behaviour::TinyFactory)), Err(AraError::Factory(_))));
    assert!(matches!(Document::new(factory_ptr(Behaviour::TinyFactory), "tiny"), Err(AraError::Factory(_))));
    assert!(
        matches!(Document::new(factory_ptr(Behaviour::NullCreate), "nullcreate"), Err(AraError::Factory(m)) if m.contains("createDocumentController"))
    );
    assert!(report("ara1").is_none() && report("tiny").is_none() && report("nullcreate").is_none(), "no controller was asked for");
}

#[test]
fn unusable_controllers_are_refused_and_cleaned_up() {
    assert!(matches!(Document::new(factory_ptr(Behaviour::NoController), "nocontroller"), Err(AraError::Controller(_))));
    assert!(matches!(Document::new(factory_ptr(Behaviour::NullInterface), "nullinterface"), Err(AraError::Controller(_))));
    let tiny = Document::new(factory_ptr(Behaviour::TinyInterface), "tinyinterface");
    assert!(matches!(tiny, Err(AraError::Controller(_))));
    // The controller with a 16-byte interface still had its destroy function there: it was used.
    assert!(report("tinyinterface").unwrap().destroyed);
    let missing = Document::new(factory_ptr(Behaviour::MissingFunction), "missing");
    assert!(matches!(&missing, Err(AraError::Controller(m)) if m.contains("createPlaybackRegion")), "{:?}", missing.err());
    let r = no_violations("missing");
    assert!(r.destroyed);
}

#[test]
fn a_session_becomes_an_ara_document_whose_audio_the_plug_in_reads() {
    let (a, b) = (tone(2, 30_000, 0.0), tone(1, 12_345, 3.0));
    let mut doc = Document::new(factory_ptr(Behaviour::Good), "mapping").unwrap();
    assert_eq!(doc.generation(), kARAAPIGeneration_2_3_Final);
    let rep = doc.sync(&graph("mapping", &a, &b));
    assert_eq!(rep.created, 2 + 2 + 3);
    assert_eq!(doc.counts(), (2, 2, 3, 3, 0));
    let r = no_violations("mapping");
    assert_eq!(r.live, (2, 2, 3, 3));
    // Every source read in full, 32- and 64-bit, matching the file sample for sample.
    let (sa, sb) = (checksum(&as_f64(&a)), checksum(&as_f64(&b)));
    assert_eq!(r.read.get("src-10"), Some(&(sa, sa, 30_000, true)));
    assert_eq!(r.read.get("src-11"), Some(&(sb, sb, 12_345, true)));
    // The same, through ARA's content reading: the plug-in's "notes".
    let notes = doc.source_notes(10).expect("notes content");
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].start, (sa >> 11) as f64);
    assert_eq!((notes[0].pitch, notes[0].duration, notes[0].volume), (2, 30_000.0, 0.5));
    assert_eq!(notes[1].volume, 1.0);
    // Analysis progress arrives through notifyModelUpdates.
    let events = doc.idle();
    assert!(events.contains(&HostEvent::Analysis { source: 10, state: AnalysisState::Completed, progress: 1.0 }), "{events:?}");
    assert!(events.contains(&HostEvent::SourceContentChanged(11)));
    assert_eq!(doc.analysis(11), Some((AnalysisState::Completed, 1.0)));
    let stats = doc.read_stats();
    assert_eq!(stats.failed, 0);
    assert_eq!(stats.open_readers, 0, "the plug-in destroyed its readers");
    assert!(stats.samples >= 2 * (2 * 30_000 + 12_345));
    // Nothing changed: no editing cycle at all.
    let calls = report("mapping").unwrap().calls.len();
    assert!(!doc.sync(&graph("mapping", &a, &b)).changed());
    assert_eq!(report("mapping").unwrap().calls.len(), calls);
    drop(doc);
    let r = no_violations("mapping");
    assert!(r.destroyed);
    assert_eq!(r.live, (0, 0, 0, 0));
}

#[test]
fn edits_and_undo_follow_the_session() {
    let (a, b) = (tone(1, 40_000, 0.5), tone(2, 20_000, 1.5));
    let mut doc = Document::new(factory_ptr(Behaviour::Good), "edits").unwrap();
    let g0 = graph("edits", &a, &b);
    doc.sync(&g0);
    // Move and trim a clip: its region is updated, nothing made or destroyed.
    let mut g1 = g0.clone();
    g1.regions[0].start_in_song = 4.0;
    g1.regions[0].start_in_source = 0.1;
    g1.regions[0].duration = 0.2;
    g1.regions[0].source_duration = 0.2;
    let rep = doc.sync(&g1);
    assert_eq!((rep.created, rep.removed, rep.updated), (0, 0, 1));
    // Delete a clip: its region goes, its modification is kept, deactivated.
    let mut g2 = g1.clone();
    g2.regions.remove(1);
    let rep = doc.sync(&g2);
    assert_eq!(rep.removed, 1);
    assert_eq!(doc.counts(), (2, 2, 2, 2, 1));
    // Delete the only clip of the second file: that source is deactivated too.
    let mut g3 = g2.clone();
    g3.regions.retain(|r| r.source != 11);
    let rep = doc.sync(&g3);
    assert_eq!(rep.deactivated, 1);
    assert_eq!(doc.counts(), (2, 1, 1, 1, 2));
    assert_eq!(report("edits").unwrap().deactivated, (1, 2));
    // Undo twice: everything comes back, reactivated rather than made anew.
    let rep = doc.sync(&g2);
    assert_eq!((rep.reactivated, rep.created), (2, 0), "{rep:?}");
    let rep = doc.sync(&g1);
    assert_eq!((rep.reactivated, rep.created), (1, 0), "{rep:?}");
    assert_eq!(doc.counts(), (2, 2, 3, 3, 0));
    // A clip that now reads another file gets a new modification on that source.
    let mut g4 = g1.clone();
    g4.regions[2].source = 10;
    g4.regions[2].sequence = 1;
    let rep = doc.sync(&g4);
    assert_eq!(rep.created, 1, "{rep:?}");
    assert_eq!(doc.counts(), (2, 1, 3, 3, 0));
    // Removing a track (and its clips) destroys its region sequence.
    let mut g5 = g4.clone();
    g5.sequences.retain(|s| s.key != 2);
    doc.sync(&g5);
    assert_eq!(doc.counts().0, 1);
    // A renamed track and a renamed session are updated in place.
    let mut g6 = g5.clone();
    g6.sequences[0].name = "Lead Vox".into();
    g6.name = "edits".into();
    assert_eq!(doc.sync(&g6).updated, 1);
    drop(doc);
    let r = no_violations("edits");
    assert!(r.destroyed && r.live == (0, 0, 0, 0) && r.deactivated == (0, 0));
}

#[test]
fn a_file_that_changes_is_read_again() {
    let a = tone(1, 10_000, 0.0);
    let mut doc = Document::new(factory_ptr(Behaviour::Good), "changes").unwrap();
    let mut g = Graph {
        name: "changes".into(),
        sequences: vec![seq(1, "A")],
        sources: vec![source(10, a.clone())],
        regions: vec![region(1, 10, 1, 0.0)],
        ..Graph::default()
    };
    doc.sync(&g);
    // The same file, longer (re-recorded or replaced): reading is turned off around the change.
    let longer = tone(1, 15_000, 0.25);
    g.sources[0].audio = longer.clone();
    assert!(doc.sync(&g).updated >= 1);
    let r = no_violations("changes");
    let s = checksum(&as_f64(&longer));
    assert_eq!(r.read.get("src-10"), Some(&(s, s, 15_000, true)));
}

#[test]
fn invalid_parts_of_a_graph_are_left_out() {
    let a = tone(1, 1000, 0.0);
    let empty = Arc::new(MemorySource { channels: Vec::new(), rate: 48_000.0 });
    let g = Graph {
        name: "x".into(),
        sequences: vec![seq(1, "A"), seq(1, "dup")],
        sources: vec![source(10, a.clone()), source(11, empty)],
        regions: vec![
            region(1, 10, 1, 0.0),
            region(2, 11, 1, 0.0),
            region(3, 10, 9, 0.0),
            RegionDesc { duration: 0.0, ..region(4, 10, 1, 0.0) },
            RegionDesc { start_in_song: f64::NAN, ..region(5, 10, 1, 0.0) },
            region(1, 10, 1, 1.0),
        ],
        tempo: vec![(0.0, 0.0)],
        bars: vec![(4, 4, 0.0)],
    };
    let v = g.valid();
    assert_eq!(v.sequences.len(), 1);
    assert_eq!(v.sources.len(), 1);
    assert_eq!(v.regions.iter().map(|r| r.key).collect::<Vec<_>>(), vec![1]);
    assert!(v.tempo.is_empty() && v.bars.is_empty(), "one sync point is not a tempo map");
}

#[test]
fn the_tempo_map_is_offered_as_musical_context_content() {
    let a = tone(1, 1000, 0.0);
    let mut doc = Document::new(factory_ptr(Behaviour::Good), "tempo").unwrap();
    let mut g = Graph {
        name: "tempo".into(),
        sequences: vec![seq(1, "A")],
        sources: vec![source(10, a)],
        regions: vec![region(1, 10, 1, 0.0)],
        ..Graph::default()
    };
    doc.sync(&g);
    assert_eq!(report("tempo").unwrap().tempo_entries, None);
    g.tempo = vec![(0.0, 0.0), (2.0, 4.0), (3.0, 6.0)];
    g.bars = vec![(4, 4, 0.0)];
    doc.sync(&g);
    assert_eq!(report("tempo").unwrap().tempo_entries, Some(3));
    no_violations("tempo");
}

#[test]
fn a_plug_in_that_refuses_sources_does_not_break_the_host() {
    let a = tone(1, 1000, 0.0);
    let mut doc = Document::new(factory_ptr(Behaviour::RefusesSources), "refuses").unwrap();
    let g = Graph {
        name: "refuses".into(),
        sequences: vec![seq(1, "A")],
        sources: vec![source(10, a)],
        regions: vec![region(1, 10, 1, 0.0)],
        ..Graph::default()
    };
    doc.sync(&g);
    assert_eq!(doc.counts(), (1, 0, 0, 0, 0));
    assert_eq!(doc.source_notes(10), None);
    drop(doc);
    no_violations("refuses");
}

#[test]
fn hostile_calls_into_the_host_are_refused() {
    let a = tone(2, 5000, 0.0);
    let mut doc = Document::new(factory_ptr(Behaviour::Hostile), "hostile").unwrap();
    let g = Graph {
        name: "hostile".into(),
        sequences: vec![seq(1, "A")],
        sources: vec![source(10, a)],
        regions: vec![region(1, 10, 1, 0.0)],
        ..Graph::default()
    };
    doc.sync(&g);
    doc.idle();
    let r = no_violations("hostile");
    assert!(r.hostile.len() >= 11, "{:?}", r.hostile);
    for (what, ok) in &r.hostile {
        assert!(ok, "{what}");
    }
    assert!(doc.read_stats().failed >= 6);
}

#[test]
fn concurrent_readers_read_the_same_audio() {
    let a = tone(2, 50_000, 0.7);
    let mut doc = Document::new(factory_ptr(Behaviour::Concurrent), "concurrent").unwrap();
    let g = Graph {
        name: "concurrent".into(),
        sequences: vec![seq(1, "A")],
        sources: vec![source(10, a.clone())],
        regions: vec![region(1, 10, 1, 0.0)],
        ..Graph::default()
    };
    doc.sync(&g);
    let r = no_violations("concurrent");
    let s = checksum(&as_f64(&a));
    assert_eq!(r.read.get("src-10"), Some(&(s, s, 50_000, true)));
}

#[test]
fn the_audio_access_controller_serves_exact_samples() {
    let a = tone(2, 7000, 0.3);
    let mut doc = Document::new(factory_ptr(Behaviour::Good), "access").unwrap();
    let g = Graph {
        name: "access".into(),
        sequences: vec![seq(1, "A")],
        sources: vec![source(10, a.clone())],
        regions: vec![region(1, 10, 1, 0.0)],
        ..Graph::default()
    };
    doc.sync(&g);
    let src = testing::source_ref(&doc, 10).unwrap();
    for use64 in [false, true] {
        let r = testing::create_reader(&doc, src, use64);
        assert!(!r.is_null());
        // Windows inside, across the start and across the end; out of range reads as silence.
        for (pos, n) in [(0i64, 7000usize), (123, 1000), (-50, 100), (6990, 40), (9000, 5), (0, 0), (0, 40_000)] {
            let (ok, got) = testing::read(&doc, r, pos, n, 2, use64);
            assert!(ok, "{pos} {n}");
            for (ch, buf) in got.iter().enumerate() {
                for (i, v) in buf.iter().enumerate() {
                    let at = pos + i as i64;
                    let want = if (0..7000).contains(&at) { f64::from(a.channels[ch][at as usize]) } else { 0.0 };
                    assert_eq!(*v, want, "{use64} ch {ch} at {at}");
                }
            }
        }
        testing::destroy_reader(&doc, r);
        let (ok, got) = testing::read(&doc, r, 0, 4, 2, use64);
        assert!(!ok, "destroyed");
        assert_eq!(got[0][0], 7.0, "a refused read writes nothing");
    }
    // Refusals.
    assert!(testing::create_reader(&doc, std::ptr::without_provenance_mut(9999), false).is_null());
    let r = testing::create_reader(&doc, src, false);
    assert!(!testing::read_raw(&doc, r, 0, 4, std::ptr::null()));
    assert!(!testing::read_raw(&doc, r, 0, -1, std::ptr::null()));
    // Several readers on several threads at once.
    let key = testing::audio_access(&doc).0 as usize;
    let readers: Vec<usize> = (0..6).map(|_| testing::create_reader(&doc, src, true) as usize).collect();
    let iface = testing::audio_access(&doc).1;
    let want: Vec<Vec<f64>> = as_f64(&a);
    std::thread::scope(|s| {
        for &rd in &readers {
            let want = &want;
            s.spawn(move || {
                let read = { iface.readAudioSamples }.unwrap();
                for start in (0..7000).step_by(333) {
                    let mut b = [vec![0.0f64; 333], vec![0.0f64; 333]];
                    let ptrs = [b[0].as_mut_ptr().cast::<std::ffi::c_void>(), b[1].as_mut_ptr().cast()];
                    // SAFETY: the host's function; two buffers of 333 doubles.
                    let ok = unsafe { read(std::ptr::without_provenance_mut(key), std::ptr::without_provenance_mut(rd), start, 333, ptrs.as_ptr()) };
                    assert_eq!(ok, kARATrue);
                    for ch in 0..2 {
                        for (i, v) in b[ch].iter().enumerate() {
                            let at = start as usize + i;
                            assert_eq!(*v, want[ch].get(at).copied().unwrap_or(0.0));
                        }
                    }
                }
            });
        }
    });
    for r in readers {
        testing::destroy_reader(&doc, std::ptr::without_provenance_mut(r));
    }
    assert_eq!(doc.read_stats().open_readers, 1);
    testing::destroy_reader(&doc, r);
    let hi = testing::host_instance(&doc);
    assert_eq!({ hi.structSize }, std::mem::size_of::<ARADocumentControllerHostInstance>());
    drop(doc);
    no_violations("access");
}

#[test]
fn a_dropped_document_answers_nothing() {
    let a = tone(1, 1000, 0.0);
    let mut doc = Document::new(factory_ptr(Behaviour::Good), "dropped").unwrap();
    let g = Graph {
        name: "dropped".into(),
        sequences: vec![seq(1, "A")],
        sources: vec![source(10, a)],
        regions: vec![region(1, 10, 1, 0.0)],
        ..Graph::default()
    };
    doc.sync(&g);
    let (ctrl, iface) = testing::audio_access(&doc);
    let src = testing::source_ref(&doc, 10).unwrap();
    drop(doc);
    let create = { iface.createAudioReaderForSource }.unwrap();
    // SAFETY: the host's own function; a controller ref of a document that is gone.
    assert!(unsafe { create(ctrl, src, kARAFalse) }.is_null());
    no_violations("dropped");
}

#[test]
fn the_log_keeps_what_happened() {
    let _ = Document::new(factory_ptr(Behaviour::Ara1), "logged");
    assert!(recent_log().iter().any(|l| l.contains("FreeDAWm Test ARA")));
}
