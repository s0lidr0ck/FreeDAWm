//! `file.bounce_movie`: the picture and the main mix as one QuickTime movie (see
//! `crate::movie_export`).

use super::*;
use crate::cmd;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        noundo "file.bounce_movie",
        "Movie (ProRes)...",
        ["File", "Export"],
        None,
        "{path (.mov), start?, end? (default: the loop when one exists, else the whole session), profile?: proxy|lt|standard|hq (default standard)} — the session's picture (its Video tracks) and main mix as a QuickTime movie: ProRes 422 at the first movie's size and frame rate, 24-bit PCM stereo; black where no Video clip plays → {path, bytes, frames, width, height, fps, black_frames}",
        always,
        bounce_movie
    )]
}

#[cfg(not(target_arch = "wasm32"))]
fn bounce_movie(e: &mut Engine, p: &Value) -> Result<Value> {
    let job = movie_job(e, p)?;
    let path = job.path().to_string_lossy().into_owned();
    let done = job.run()?;
    Ok(
        json!({"path": path, "bytes": done.bytes, "frames": done.frames, "width": done.width, "height": done.height, "fps": done.fps, "black_frames": done.black_frames}),
    )
}

#[cfg(target_arch = "wasm32")]
fn bounce_movie(_: &mut Engine, _: &Value) -> Result<Value> {
    Err(bad("file.bounce_movie", "the browser build cannot write movies"))
}

/// The movie `file.bounce_movie` writes for `p`, not rendered yet.
#[cfg(not(target_arch = "wasm32"))]
pub fn movie_job(e: &Engine, p: &Value) -> Result<crate::movie_export::MovieBounce> {
    const C: &str = "file.bounce_movie";
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(C, "`path` required"))?.to_string();
    let path = std::path::Path::new(&path).with_extension("mov");
    let r = match (p.get("start").is_some() || p.get("end").is_some() || p.get("length").is_some(), e.session().edit.loop_range) {
        (true, _) => range_param(e, C, p)?,
        (false, Some(lp)) => lp,
        (false, None) => soundcraft_time::Range::default(),
    };
    let r = if r.is_empty() { soundcraft_time::Range::new(0, e.session().content_end().max(1)) } else { r };
    let profile = crate::movie_export::profile(str_param(p, "profile").unwrap_or("standard"))
        .ok_or_else(|| bad(C, "`profile` is proxy, lt, standard or hq"))?;
    crate::movie_export::MovieBounce::new(e.session_arc(), &path, r, profile)
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use soundcraft_video::isobmff::{TrackKind, open};

    /// A session with a ProRes movie on a Video track and a tone on an audio track bounces to a
    /// movie with a picture track at the movie's size and rate, and the mix as PCM beside it.
    #[test]
    fn a_movie_bounce_has_the_picture_and_the_mix() {
        let dir = std::env::temp_dir().join(format!("sc-bounce-movie-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let movie = dir.join("pic.mov");
        std::fs::write(&movie, soundcraft_video::synth::prores_mov(25).unwrap()).unwrap();
        let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let wav = dir.join("tone.wav");
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
        std::fs::write(&wav, soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).unwrap()).unwrap();
        let mut e = Engine::default();
        e.execute("file.import_video", &json!({"path": movie.to_string_lossy()})).unwrap();
        e.execute("file.import_audio", &json!({"path": wav.to_string_lossy()})).unwrap();
        let out = dir.join("out.mov");
        let r = e.execute("file.bounce_movie", &json!({"path": out.to_string_lossy(), "start": 0, "end": 48_000, "profile": "proxy"})).unwrap();
        assert_eq!((r["width"].as_u64(), r["height"].as_u64(), r["fps"].as_f64()), (Some(64), Some(48), Some(25.0)), "{r}");
        assert_eq!(r["frames"].as_u64(), Some(25));
        assert_eq!(r["black_frames"].as_u64(), Some(0), "{r}");
        let bytes = std::fs::read(&out).unwrap();
        assert!(!dir.join("out.mov.part").exists());
        let f = open(bytes.as_slice()).unwrap();
        let video = f.tracks.iter().find(|t| t.kind == TrackKind::Video).expect("a picture track");
        assert_eq!(video.samples.len(), 25);
        let audio = f.tracks.iter().find(|t| t.kind == TrackKind::Audio).expect("a sound track");
        let frames: u64 = audio.samples.iter().map(|s| u64::from(s.duration)).sum();
        assert_eq!(frames, 48_000, "every sample of the range");
        // The pictures decode, and the sound is not silent.
        let mut m = soundcraft_video::Movie::open(bytes.clone()).unwrap();
        assert_eq!(m.frame_at(0.5).unwrap().width, 64);
        let some = f.read_sample(bytes.as_slice(), f.tracks.iter().position(|t| t.kind == TrackKind::Audio).unwrap(), 10).unwrap();
        assert!(some.iter().any(|b| *b != 0));
        // Nothing to show: refused, nothing written.
        let mut plain = Engine::default();
        plain.execute("file.import_audio", &json!({"path": wav.to_string_lossy()})).unwrap();
        let none = dir.join("none.mov");
        assert!(plain.execute("file.bounce_movie", &json!({"path": none.to_string_lossy()})).is_err());
        assert!(!none.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
