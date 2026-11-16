//! Bounce to Movie: the session's picture (its Video tracks, as the Video window shows them) and
//! its main mix, written together as a QuickTime movie: ProRes 422 video at the movie's frame
//! rate and size, and 24-bit PCM stereo audio. Rendered a frame at a time ([`MovieBounce::step`])
//! so a UI can show progress; the file appears at its path only once complete.
//!
//! Design: the encoder and the muxer are the video crate's own (`prores::Encoder`,
//! `isobmff::Mp4Writer`); pictures come from the bundled decoders (`Movie::frame_at`) and are
//! converted from RGBA to 10-bit 4:2:2 (BT.709, video range). The timeline is mapped to movie
//! time as the Video window does it (clip start, clip offset, the Video Sync Offset); where no
//! Video clip plays, the frame is black. The audio is rendered by the mix engine's offline render
//! (`RangeRender`), folded to stereo when the main output is wider, and written beside each frame.

use crate::{EngineError, Result};
use soundcraft_model::{ClipContent, Session, SourceId, TrackKind};
use soundcraft_time::{Range, Samples};
use soundcraft_video::isobmff::{Brand, FourCc, Mp4Writer, PcmConfig, SampleEntry, TrackConfig, WriteSample, WriterOptions};
use soundcraft_video::prores::{ChromaFormat, Encoder, Frame as ProResFrame, Profile};
use soundcraft_video::{Movie, SharedSource};
use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Longest movie bounce, in seconds (an hour).
pub const MAX_MOVIE_SECS: f64 = 3600.0;
/// Largest picture written.
const MAX_SIDE: u32 = 4096;

/// A frame rate as a rational: `num / den` frames per second (29.97 is 30000/1001).
pub fn frame_rate(fps: f64) -> (u32, u32) {
    let fps = if (1.0..=240.0).contains(&fps) { fps } else { 25.0 };
    let ntsc = fps * 1001.0 / 1000.0;
    if (ntsc - ntsc.round()).abs() < 0.005 && (fps - fps.round()).abs() > 0.005 {
        ((ntsc.round() as u32) * 1000, 1001)
    } else {
        ((fps.round() as u32).max(1), 1)
    }
}

/// The ProRes flavour by name: proxy, lt, standard (the default) or hq.
pub fn profile(name: &str) -> Option<Profile> {
    match name.trim().to_ascii_lowercase().as_str() {
        "proxy" => Some(Profile::Proxy),
        "lt" => Some(Profile::Lt),
        "" | "standard" | "422" => Some(Profile::Standard),
        "hq" => Some(Profile::Hq),
        _ => None,
    }
}

/// The picture at timeline position `at`: the topmost active Video track with an unmuted clip
/// there, as (movie, seconds into it).
pub fn picture_at(s: &Session, at: Samples) -> Option<(SourceId, f64)> {
    let sync = s.edit.value("video.sync_offset", 0.0);
    let sync = if sync.is_finite() { sync } else { 0.0 };
    let rate = s.sample_rate.as_f64().max(1.0);
    for t in s.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.inactive) {
        for c in t.clips() {
            if c.muted || !c.range().contains(at) {
                continue;
            }
            if let ClipContent::Video { source, offset } = c.content
                && s.video(source).is_some()
            {
                let secs = (at.saturating_sub(c.start).saturating_add(offset) as f64 - sync) / rate;
                return Some((source, secs.max(0.0)));
            }
        }
    }
    None
}

/// RGBA (8-bit) to 10-bit 4:2:2, BT.709 video range, into `f` (same size).
pub fn rgba_to_yuv422(rgba: &[u8], f: &mut ProResFrame) {
    let (w, h) = (f.width as usize, f.height as usize);
    let cw = f.chroma_width() as usize;
    let px = |x: usize, y: usize| -> [f32; 3] {
        let i = (y * w + x) * 4;
        match rgba.get(i..i + 3) {
            Some(p) => [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0],
            None => [0.0; 3],
        }
    };
    let ten = |v: f32| v.round().clamp(64.0, 940.0) as u16;
    let ten_c = |v: f32| v.round().clamp(64.0, 960.0) as u16;
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = px(x, y);
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            if let Some(o) = f.y.get_mut(y * w + x) {
                *o = ten(64.0 + 876.0 * luma);
            }
        }
        for cx in 0..cw {
            // The two pixels a chroma sample covers, averaged.
            let (a, b2) = (px((cx * 2).min(w.saturating_sub(1)), y), px((cx * 2 + 1).min(w.saturating_sub(1)), y));
            let [r, g, b] = [(a[0] + b2[0]) / 2.0, (a[1] + b2[1]) / 2.0, (a[2] + b2[2]) / 2.0];
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let (pb, pr) = ((b - luma) / 1.8556, (r - luma) / 1.5748);
            if let Some(o) = f.cb.get_mut(y * cw + cx) {
                *o = ten_c(512.0 + 896.0 * pb);
            }
            if let Some(o) = f.cr.get_mut(y * cw + cx) {
                *o = ten_c(512.0 + 896.0 * pr);
            }
        }
    }
}

/// A finished movie bounce.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovieBounced {
    pub bytes: u64,
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Frames with no picture (no Video clip there, or the movie could not be read).
    pub black_frames: u64,
}

/// A movie bounce in progress (see the module docs).
pub struct MovieBounce {
    session: Arc<Session>,
    path: PathBuf,
    part: PathBuf,
    range: Range,
    rate: (u32, u32),
    size: (u32, u32),
    frames: u64,
    done: u64,
    black: u64,
    movies: HashMap<SourceId, std::result::Result<Movie, String>>,
    encoder: Encoder,
    picture: ProResFrame,
    writer: Option<Mp4Writer<BufWriter<std::fs::File>>>,
    video_track: usize,
    audio_track: usize,
    render: soundcraft_mix::RangeRender,
    fold: Option<soundcraft_model::ChannelFormat>,
    /// Audio samples written so far.
    audio_done: i64,
    pcm: Vec<u8>,
}

fn io_err(path: &Path, e: impl std::fmt::Display) -> EngineError {
    EngineError::Io(format!("{}: {e}", path.display()))
}

impl MovieBounce {
    /// Starts a movie of `s` over `r` to `path` (a `.mov`), in ProRes `profile`. The size and
    /// frame rate are the first movie's that plays in the range.
    pub fn new(s: Arc<Session>, path: &Path, r: Range, profile: Profile) -> Result<MovieBounce> {
        let bad = |m: &str| EngineError::BadParams("file.bounce_movie".into(), m.into());
        let rate_hz = s.sample_rate.as_f64().max(1.0);
        if r.is_empty() {
            return Err(bad("nothing to bounce: the range is empty"));
        }
        if r.len() as f64 / rate_hz > MAX_MOVIE_SECS {
            return Err(bad("movie bounces are limited to an hour"));
        }
        // The first movie seen in the range sets the size and frame rate.
        let step = (rate_hz / 10.0).max(1.0) as Samples;
        let first = (0..)
            .map(|k: i64| r.start.saturating_add(k.saturating_mul(step)))
            .take_while(|at| *at < r.end)
            .find_map(|at| picture_at(&s, at))
            .and_then(|(src, _)| s.video(src))
            .ok_or_else(|| bad("no Video track plays a movie in that range: use Bounce Mix for the audio alone"))?;
        let even = |v: u32| (v.clamp(16, MAX_SIDE)) & !1;
        let size = (even(first.width), even(first.height));
        let rate = frame_rate(first.frame_rate);
        let fps = f64::from(rate.0) / f64::from(rate.1);
        let frames = ((r.len() as f64 / rate_hz) * fps).ceil().max(1.0) as u64;
        let name = path.file_name().map_or_else(|| "movie".into(), |n| n.to_string_lossy().into_owned());
        let part = path.with_file_name(format!("{name}.part"));
        crate::file_access::check_write(path).map_err(EngineError::Io)?;
        crate::file_access::check_write(&part).map_err(EngineError::Io)?;
        let file = std::fs::File::create(&part).map_err(|e| io_err(path, e))?;
        let mut w = Mp4Writer::new(BufWriter::new(file), WriterOptions::new(Brand::Mov)).map_err(|e| io_err(path, e))?;
        let fourcc = FourCc(profile.fourcc());
        let video_track =
            w.add_track(TrackConfig::new(SampleEntry::prores(fourcc, size.0 as u16, size.1 as u16), rate.0)).map_err(|e| io_err(path, e))?;
        let pcm = PcmConfig { bits: 24, float: false, big_endian: false, signed: true, channels: 2, sample_rate: rate_hz };
        let audio_track = w.add_track(TrackConfig::new(SampleEntry::pcm(pcm), s.sample_rate.hz())).map_err(|e| io_err(path, e))?;
        let nch = soundcraft_mix::source_channels(&s, None);
        let fold = (nch > 2).then(|| s.main_format());
        let render = soundcraft_mix::RangeRender::new(&s, None, r, 1024);
        Ok(MovieBounce {
            path: path.to_path_buf(),
            part,
            range: r,
            rate,
            size,
            frames,
            done: 0,
            black: 0,
            movies: HashMap::new(),
            encoder: Encoder::new(profile, size.0, size.1),
            picture: ProResFrame::new(size.0, size.1, ChromaFormat::Yuv422, 10, false),
            writer: Some(w),
            video_track,
            audio_track,
            render,
            fold,
            audio_done: 0,
            pcm: Vec::new(),
            session: s,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How far along, from 0 to 1.
    pub fn progress(&self) -> f32 {
        (self.done as f64 / self.frames.max(1) as f64).min(1.0) as f32
    }

    /// Timeline position where frame `k` starts.
    fn frame_pos(&self, k: u64) -> Samples {
        let secs = k as f64 * f64::from(self.rate.1) / f64::from(self.rate.0);
        self.range.start.saturating_add((secs * self.session.sample_rate.as_f64()).round() as Samples)
    }

    /// The picture of frame `k` into `self.picture` (black when there is none).
    fn draw(&mut self, k: u64) {
        let at = self.frame_pos(k);
        let rgba = picture_at(&self.session, at).and_then(|(src, secs)| {
            let movie = self.movies.entry(src).or_insert_with(|| open_movie(&self.session, src));
            let m = movie.as_mut().ok()?;
            m.frame_at(secs).ok()
        });
        match rgba {
            Some(fr) => {
                let (w, h) = self.size;
                if (fr.width, fr.height) == (w, h) {
                    rgba_to_yuv422(&fr.rgba, &mut self.picture);
                } else {
                    let scaled = soundcraft_video::convert::scale_rgba(&fr.rgba, fr.width as usize, fr.height as usize, w as usize, h as usize);
                    rgba_to_yuv422(&scaled, &mut self.picture);
                }
            }
            None => {
                self.black += 1;
                self.picture.y.iter_mut().for_each(|v| *v = 64);
                self.picture.cb.iter_mut().for_each(|v| *v = 512);
                self.picture.cr.iter_mut().for_each(|v| *v = 512);
            }
        }
    }

    /// Renders and writes the mix up to timeline position `until`.
    fn audio_until(&mut self, until: Samples) -> Result<()> {
        let want = until.saturating_sub(self.range.start).min(self.range.len());
        while self.audio_done < want {
            let Some((chunk, n)) = self.render.next_block(&self.session) else { break };
            if n == 0 {
                continue;
            }
            let folded = self.fold.map(|from| soundcraft_mix::fold_down_stereo(from, chunk));
            let src = folded.as_deref().unwrap_or(chunk);
            self.pcm.clear();
            for i in 0..n {
                for c in 0..2 {
                    let v = src.get(c).or_else(|| src.first()).and_then(|ch| ch.get(i)).copied().unwrap_or(0.0);
                    let v = if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
                    let q = (f64::from(v) * 8_388_607.0).round() as i32;
                    self.pcm.extend_from_slice(&q.to_le_bytes()[..3]);
                }
            }
            let Some(w) = self.writer.as_mut() else { break };
            w.write_sample(self.audio_track, WriteSample { data: &self.pcm, duration: 0, composition_offset: 0, is_sync: true })
                .map_err(|e| io_err(&self.path, e))?;
            self.audio_done += n as i64;
        }
        Ok(())
    }

    /// Writes frames until `more` returns false (at least one per call). Returns the finished
    /// movie once its file is complete.
    pub fn step(&mut self, mut more: impl FnMut() -> bool) -> Result<Option<MovieBounced>> {
        loop {
            if self.done >= self.frames {
                return self.finish().map(Some);
            }
            let k = self.done;
            self.draw(k);
            let data = self.encoder.encode(&self.picture).map_err(|e| io_err(&self.path, e))?;
            let Some(w) = self.writer.as_mut() else { return Err(io_err(&self.path, "already finished")) };
            w.write_sample(self.video_track, WriteSample { data: &data, duration: self.rate.1, composition_offset: 0, is_sync: true })
                .map_err(|e| io_err(&self.path, e))?;
            self.done += 1;
            let end = self.frame_pos(self.done).min(self.range.end);
            self.audio_until(end)?;
            if !more() {
                return Ok(None);
            }
        }
    }

    /// Renders the whole movie (see [`MovieBounce::step`]).
    pub fn run(mut self) -> Result<MovieBounced> {
        loop {
            if let Some(done) = self.step(|| true)? {
                return Ok(done);
            }
        }
    }

    fn finish(&mut self) -> Result<MovieBounced> {
        self.audio_until(self.range.end)?;
        let w = self.writer.take().ok_or_else(|| io_err(&self.path, "already finished"))?;
        let mut out = w.finish().map_err(|e| io_err(&self.path, e))?;
        out.flush().map_err(|e| io_err(&self.path, e))?;
        drop(out);
        std::fs::rename(&self.part, &self.path).map_err(|e| io_err(&self.path, e))?;
        let bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        Ok(MovieBounced {
            bytes,
            frames: self.frames,
            width: self.size.0,
            height: self.size.1,
            fps: f64::from(self.rate.0) / f64::from(self.rate.1),
            black_frames: self.black,
        })
    }
}

impl Drop for MovieBounce {
    /// Removes the part file unless the movie completed (which renamed it).
    fn drop(&mut self) {
        drop(self.writer.take());
        let _ = std::fs::remove_file(&self.part);
    }
}

fn open_movie(s: &Session, src: SourceId) -> std::result::Result<Movie, String> {
    let v = s.video(src).ok_or("no such movie")?;
    if let Some(why) = &v.offline {
        return Err(why.clone());
    }
    crate::file_access::check_read(Path::new(&v.path))?;
    let f = std::fs::File::open(&v.path).map_err(|e| format!("{}: {e}", v.path))?;
    let src: SharedSource = Arc::new(f);
    Movie::open_source(src).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rates_are_rational() {
        assert_eq!(frame_rate(25.0), (25, 1));
        assert_eq!(frame_rate(29.97), (30000, 1001));
        assert_eq!(frame_rate(23.976), (24000, 1001));
        assert_eq!(frame_rate(30.0), (30, 1));
        assert_eq!(frame_rate(f64::NAN), (25, 1));
    }

    #[test]
    fn grey_and_colour_convert_to_video_range() {
        let mut f = ProResFrame::new(2, 1, ChromaFormat::Yuv422, 10, false);
        rgba_to_yuv422(&[255, 255, 255, 255, 255, 255, 255, 255], &mut f);
        assert_eq!((f.y[0], f.cb[0], f.cr[0]), (940, 512, 512));
        rgba_to_yuv422(&[0, 0, 0, 255, 0, 0, 0, 255], &mut f);
        assert_eq!((f.y[0], f.cb[0], f.cr[0]), (64, 512, 512));
        rgba_to_yuv422(&[255, 0, 0, 255, 255, 0, 0, 255], &mut f);
        assert!(f.cr[0] > 900 && f.cb[0] < 512, "red: Cr high, Cb low ({}, {})", f.cb[0], f.cr[0]);
    }
}
