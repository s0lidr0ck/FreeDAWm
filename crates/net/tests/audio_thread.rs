//! No network work on the audio thread: the audio side (playback, mix) knows nothing of the
//! network, and the network reads the transport only through `TakeClock` (atomics behind it).

fn read_tree(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            read_tree(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs" || x == "toml") {
            out.push((p.display().to_string(), std::fs::read_to_string(&p).unwrap_or_default()));
        }
    }
}

#[test]
fn the_audio_side_has_no_network_code() {
    // Read at run time (see AGENTS.md on CARGO_MANIFEST_DIR).
    let here = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let crates = here.parent().unwrap();
    for c in ["playback", "mix"] {
        let mut files = Vec::new();
        read_tree(&crates.join(c), &mut files);
        assert!(!files.is_empty());
        for (path, text) in files {
            for needle in ["soundcraft-net", "soundcraft_net", "TcpStream", "UdpSocket", "TcpListener", "mdns"] {
                assert!(!text.contains(needle), "{path} mentions {needle}");
            }
        }
    }
    // The network never holds the player: it reads a clock, and the mix stream's two lock-free
    // ends (`soundcraft_playback::stream`: a tap it reads, a queue it fills), nothing else.
    let mut files = Vec::new();
    read_tree(&here.join("src"), &mut files);
    for (path, text) in files {
        for (i, _) in text.match_indices("soundcraft_playback") {
            let rest = text.get(i + "soundcraft_playback".len()..).unwrap_or("");
            assert!(rest.starts_with("::stream"), "{path} reaches into the player: {}", rest.chars().take(40).collect::<String>());
        }
    }
    // Those two ends take no lock and make no thread on the audio side.
    let stream = std::fs::read_to_string(crates.join("playback").join("src").join("stream.rs")).unwrap();
    for needle in ["Mutex", "RwLock", "thread::spawn", "Condvar"] {
        assert!(!stream.contains(needle), "playback/src/stream.rs uses {needle}");
    }
}
