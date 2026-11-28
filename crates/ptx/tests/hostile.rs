//! Damaged and crafted files: every one is an error or a partial result, never a panic, and each
//! is read within a time bound (a hang or a quadratic scan fails the test).

use soundcraft_ptx::{MAX_PTX_BYTES, PtxError, decode_bytes};
use std::time::{Duration, Instant};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

/// A file header with no scrambling (XOR type 5, value 0: the key is all zeros).
fn header() -> Vec<u8> {
    let mut h = vec![0u8; 0x14];
    h[0] = 0x03;
    h[0x12] = 5;
    h
}

/// One `0x5A` block: type, size (content type + payload), content type, payload.
fn block(bt: u16, ct: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x5A];
    out.extend_from_slice(&bt.to_le_bytes());
    out.extend_from_slice(&u32::try_from(payload.len() + 2).unwrap().to_le_bytes());
    out.extend_from_slice(&ct.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn file(blocks: &[Vec<u8>]) -> Vec<u8> {
    let mut f = header();
    for b in blocks {
        f.extend_from_slice(b);
    }
    f
}

/// Reads `bytes` and checks it took less than `limit`.
fn read_within(bytes: &[u8], limit: Duration, what: &str) -> Result<soundcraft_ptx::PtxDocument, PtxError> {
    let t = Instant::now();
    let r = decode_bytes("x.ptx", bytes);
    assert!(t.elapsed() < limit, "{what}: {:?}", t.elapsed());
    r
}

#[test]
fn a_file_cut_at_every_97th_byte_never_panics() {
    for name in ["CR_min.ptx", "CR_max3.ptx"] {
        let good = fixture(name);
        let t = Instant::now();
        for n in (0..good.len()).step_by(97) {
            let _ = read_within(&good[..n], Duration::from_secs(2), &format!("{name} cut at {n}"));
        }
        assert!(t.elapsed() < Duration::from_secs(120), "{name}: {:?}", t.elapsed());
    }
}

#[test]
fn random_bit_flips_never_panic() {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for (name, rounds) in [("CR_min.ptx", 1500), ("CR_max3.ptx", 300)] {
        let good = fixture(name);
        for _ in 0..rounds {
            let mut b = good.clone();
            for _ in 0..(1 + next() % 8) {
                let at = (next() as usize) % b.len();
                b[at] ^= 1 << (next() % 8);
            }
            let _ = read_within(&b, Duration::from_secs(2), name);
        }
    }
}

#[test]
fn empty_short_and_foreign_files_are_errors() {
    assert_eq!(decode_bytes("x.ptx", &[]), Err(PtxError::TooShort(0)));
    assert!(matches!(decode_bytes("x.ptx", &header()), Err(PtxError::TooShort(_))));
    assert!(decode_bytes("x.ptx", b"RIFF....WAVEfmt this is not a session at all").is_err());
    let mut wrong = header();
    wrong[0x12] = 2;
    wrong.extend([0u8; 64]);
    assert_eq!(decode_bytes("x.ptx", &wrong), Err(PtxError::UnknownXorType(2)));
    // A header and nothing that looks like a block.
    let mut f = header();
    f.extend([0x11u8; 100]);
    assert!(matches!(decode_bytes("x.ptx", &f), Err(PtxError::Layout { .. })));
}

#[test]
fn a_file_over_the_limit_is_refused_before_it_is_read() {
    let big = vec![0u8; MAX_PTX_BYTES + 1];
    assert_eq!(decode_bytes("x.ptx", &big), Err(PtxError::TooLarge(MAX_PTX_BYTES + 1)));
}

#[test]
fn huge_declared_counts_read_only_what_is_there() {
    // A track list declaring four billion tracks, with one.
    let mut p = vec![0u8; 10];
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(&u32::MAX.to_le_bytes());
    p.extend_from_slice(&0u16.to_le_bytes());
    p.extend_from_slice(&4u32.to_le_bytes());
    p.extend_from_slice(b"sine");
    p.extend_from_slice(&[0u8; 6]);
    p.extend_from_slice(&[0x2a, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 1, 0, 0]);
    // A tempo map of four billion events, a meter map, a marker list and a MIDI chunk with as many
    // notes, a mixer lane with a million points in 40 bytes, a folder map and a group list.
    let mut tempo = b"Tempo".to_vec();
    tempo.extend([0u8; 6]);
    tempo.extend_from_slice(&u32::MAX.to_le_bytes());
    let mut meter = b"Meter".to_vec();
    meter.extend([0u8; 6]);
    meter.extend_from_slice(&50_000u32.to_le_bytes());
    let mut midi = b"MdChun".to_vec();
    midi.extend(b"MdNLB");
    midi.extend([0u8; 6]);
    midi.extend_from_slice(&u32::MAX.to_le_bytes());
    midi.extend([0u8; 40]);
    let mut lane = vec![0x01, 0x46, 0x01, 0x00, 0, 0, 0, 0, 0, 0];
    lane.extend_from_slice(&1_000_000u32.to_le_bytes());
    lane.extend([0u8; 30]);
    let mut folders = u32::MAX.to_le_bytes().to_vec();
    folders.extend([0u8; 30]);
    let mut groups = u32::MAX.to_le_bytes().to_vec();
    groups.extend(block(1, 0x202A, &[255, 255, 255, 255]));
    let f = file(&[
        block(9, 0x2519, &p),
        block(1, 0x2028, &tempo),
        block(1, 0x2029, &meter),
        block(1, 0x2000, &midi),
        block(1, 0x260D, &block(1, 0x260A, &lane)),
        block(1, 0x210C, &folders),
        block(1, 0x202B, &groups),
    ]);
    let d = read_within(&f, Duration::from_secs(2), "huge counts").unwrap();
    assert_eq!(d.tracks.len(), 1);
    assert!(d.tempo_used.is_empty() && d.meter_used.is_empty());
    assert!(d.midi_lists.iter().all(|c| c.notes.is_empty()));
    assert!(!d.warnings.is_empty());
}

#[test]
fn deep_nesting_is_refused_without_running_out_of_stack() {
    // 200 blocks each holding the next.
    let mut inner = block(2, 0x1000, &[0u8; 4]);
    for _ in 0..200 {
        inner = block(2, 0x1000, &inner);
    }
    let r = read_within(&file(&[inner]), Duration::from_secs(2), "deep nesting");
    assert_eq!(r.err(), Some(PtxError::Limit("block nesting deeper than 64")));
    // 60 levels are fine.
    let mut inner = block(2, 0x1000, &[0u8; 4]);
    for _ in 0..60 {
        inner = block(2, 0x1000, &inner);
    }
    assert!(read_within(&file(&[inner]), Duration::from_secs(2), "60 levels").is_ok());
}

#[test]
fn too_many_blocks_are_refused() {
    let one = block(2, 0x1000, &[]);
    let mut payload = Vec::with_capacity(one.len() * 2_000_100);
    for _ in 0..2_000_100 {
        payload.extend_from_slice(&one);
    }
    let r = read_within(&file(&[block(1, 0x1001, &payload)]), Duration::from_secs(30), "many blocks");
    assert_eq!(r.err(), Some(PtxError::Limit("more than 2 000 000 blocks")));
}

#[test]
fn overlapping_midi_records_are_read_in_bounded_time() {
    // Four megabytes of controller-record headers, each claiming the rest of the chunk: the
    // upstream reader searched the rest again for each (quadratic).
    let mut midi = b"MdChun".to_vec();
    let mut rec = b"MdCDS".to_vec();
    rec.extend([0u8; 2]);
    rec.extend_from_slice(&u32::MAX.to_le_bytes());
    while midi.len() < 4 << 20 {
        midi.extend_from_slice(&rec);
    }
    let _ = read_within(&file(&[block(1, 0x2000, &midi)]), Duration::from_secs(20), "overlapping MdCDS");
    // The same with real-looking records inside, each with a controller lane.
    let mut midi = b"MdChun".to_vec();
    let mut rec = b"MdCDS".to_vec();
    rec.extend([0u8; 2]);
    rec.extend_from_slice(&u32::MAX.to_le_bytes());
    rec.extend(b"Mdss");
    rec.extend([0u8; 20]);
    rec.extend(b"MdCD");
    rec.extend([0u8; 6]);
    rec.extend_from_slice(&1u32.to_le_bytes());
    rec.extend([0u8; 12]);
    while midi.len() < 2 << 20 {
        midi.extend_from_slice(&rec);
    }
    let d = read_within(&file(&[block(1, 0x2000, &midi)]), Duration::from_secs(20), "overlapping MdCDS with lanes");
    if let Ok(d) = d {
        let points: usize = d.midi_lists.iter().flat_map(|c| &c.controllers).map(|k| k.points.len()).sum();
        assert!(points <= soundcraft_ptx::decode::midi::MAX_MIDI_POINTS);
    }
}

#[test]
fn nested_playlists_do_not_multiply_what_is_read() {
    // A playlist list nested 25 deep (50 blocks, under the depth limit), each level holding a
    // playlist with 5 000 entries: the upstream reader read the inner entries again at every level
    // above them (1.6 million entries from 125 000).
    let entry = block(10, 0x104F, &[0u8; 20]);
    let mut entries = Vec::new();
    for _ in 0..5_000 {
        entries.extend_from_slice(&entry);
    }
    let mut level = Vec::new();
    for _ in 0..25 {
        let mut pl = 4u32.to_le_bytes().to_vec();
        pl.extend(b"Trk1");
        pl.extend([0u8; 5]);
        pl.extend_from_slice(&entries);
        pl.extend_from_slice(&level);
        let mut list = vec![0u8; 4];
        list.extend(block(3, 0x1052, &pl));
        level = block(2, 0x1054, &list);
    }
    let d = read_within(&file(&[level]), Duration::from_secs(20), "nested playlists").unwrap();
    let n: usize = d.audio_playlists.iter().map(|(_, l)| l.len()).sum();
    assert!(n > 100_000 && n <= soundcraft_ptx::decode::clips::MAX_PLACEMENTS, "{n}");
}
