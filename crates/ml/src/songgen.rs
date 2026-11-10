//! Generate Song: whole songs (vocals and accompaniment, 48 kHz stereo) from YuE2 or ACE-Step 1.5,
//! through their native C++/GGML servers, `yue-server` (yue2.cpp) and `ace-server`
//! (acestep.cpp), both MIT. FreeDAWm downloads each as a song engine (Setup › ML Models: the
//! server built for this platform and its GGUF models) and runs it itself
//! ([`crate::engine_server`]); a `server` address uses a server the person runs instead.
//!
//! Both servers speak the same job protocol: `POST` a JSON request → `{"id"}`; `GET
//! /job?id=N` → `{"status": running|done|failed|cancelled}`; `GET /job?id=N&result=1` → the
//! result; `POST /job?id=N&cancel=1` cancels. Results are `multipart/mixed`: per track YuE2 sends
//! the replay request (JSON, with the score it wrote as `abc` and the seeds it used), the audio
//! and the latents; ACE-Step sends the audio and the latents. ACE-Step's `POST /lm` (the 5 Hz
//! planner: metadata, lyrics and audio codes) answers with the enriched request as JSON, which
//! then goes to `POST /synth`.
//!
//! The audio comes back as 24-bit WAV (`output_format: "wav24"`), which both servers normalise
//! (their `peak_clip` percentile, as in their WebUIs); `wav32` would come back unnormalised, as
//! the model left it (YuE2 peaked at 0.39 on a test song, ACE-Step at 0.97).

use crate::job::Ctx;
use crate::{MlError, Result};
use std::io::Read;
use std::time::Duration;

/// Which model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SongEngine {
    /// YuE2 (m-a-p): a score first (editable), then the song. Weights CC BY-NC 4.0 with a creator
    /// permission (musicians may monetise their songs).
    #[default]
    Yue2,
    /// ACE-Step 1.5: fast, MIT.
    AceStep,
}

impl SongEngine {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace(['-', '_', ' ', '.'], "").as_str() {
            "yue2" | "yue" => Some(SongEngine::Yue2),
            "acestep" | "ace" | "acestep15" => Some(SongEngine::AceStep),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SongEngine::Yue2 => "yue2",
            SongEngine::AceStep => "acestep",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SongEngine::Yue2 => "YuE2",
            SongEngine::AceStep => "ACE-Step 1.5",
        }
    }

    /// The server's address when the person gives none (each server's own default port).
    pub fn default_server(self) -> &'static str {
        match self {
            SongEngine::Yue2 => "http://127.0.0.1:8087",
            SongEngine::AceStep => "http://127.0.0.1:8080",
        }
    }
}

/// Text held inline, so job options stay `Copy`: at most `N` bytes, cut at a character boundary.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InlineText<const N: usize> {
    len: u16,
    bytes: [u8; N],
}

impl<const N: usize> Default for InlineText<N> {
    fn default() -> Self {
        InlineText { len: 0, bytes: [0; N] }
    }
}

impl<const N: usize> std::fmt::Debug for InlineText<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl<const N: usize> InlineText<N> {
    /// `text` without NULs and with `\r\n` as `\n`, trimmed at the ends, cut to `N` bytes.
    pub fn new(text: &str) -> Self {
        let clean = text.replace("\r\n", "\n").replace('\0', "");
        let clean = clean.trim();
        let mut end = clean.len().min(N).min(usize::from(u16::MAX));
        while end > 0 && !clean.is_char_boundary(end) {
            end -= 1;
        }
        let mut t = InlineText::default();
        if let (Some(dst), Some(src)) = (t.bytes.get_mut(..end), clean.as_bytes().get(..end)) {
            dst.copy_from_slice(src);
            t.len = u16::try_from(end).unwrap_or(0);
        }
        t
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(self.bytes.get(..usize::from(self.len)).unwrap_or(&[])).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether `text` (after the same cleaning) was cut to fit.
    pub fn would_cut(text: &str) -> bool {
        text.replace("\r\n", "\n").trim().len() > N
    }
}

pub const MAX_STYLE: usize = 1024;
pub const MAX_LYRICS: usize = 8192;
pub const MAX_SCORE: usize = 16384;
pub const MAX_SERVER: usize = 200;
pub const DEFAULT_SECONDS: f32 = 60.0;
pub const MAX_SECONDS: f32 = 600.0;

/// Generate Song's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SongOptions {
    pub engine: SongEngine,
    /// A server the person runs, `http://host:port`; empty: the engine FreeDAWm installs and runs
    /// ([`crate::engine_server`]).
    pub server: InlineText<MAX_SERVER>,
    /// Style: YuE2's style tags, ACE-Step's caption.
    pub style: InlineText<MAX_STYLE>,
    /// Lyrics with section tags (`[Verse]`, `[Chorus]`). Empty: YuE2 sings none; ACE-Step
    /// writes its own unless `instrumental`.
    pub lyrics: InlineText<MAX_LYRICS>,
    /// No vocals (ACE-Step: `[Instrumental]`; YuE2: no lyrics).
    pub instrumental: bool,
    /// YuE2: an ABC score to sing (empty: YuE2 writes one).
    pub score: InlineText<MAX_SCORE>,
    /// YuE2's planning: `full` (melody and chords, default), `melody`, `off`.
    pub plan: ScorePlan,
    /// Target length, seconds (10–600); the model may end the song sooner.
    pub seconds: f32,
    /// Seed (YuE2: both its seeds); 0 = the server chooses (the result reports it).
    pub seed: u64,
    /// ACE-Step: tempo, 0 = the planner picks.
    pub bpm: u16,
    /// ACE-Step: key ("C major", "F# minor"), empty = the planner picks.
    pub key: InlineText<32>,
    /// ACE-Step: run the 5 Hz planner first (metadata, lyrics, audio codes); off is the DiT alone.
    pub planner: bool,
}

/// YuE2's chain-of-thought mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScorePlan {
    #[default]
    Full,
    Melody,
    Off,
}

impl ScorePlan {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "full" => Some(ScorePlan::Full),
            "melody" => Some(ScorePlan::Melody),
            "off" | "none" => Some(ScorePlan::Off),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            ScorePlan::Full => "full",
            ScorePlan::Melody => "melody",
            ScorePlan::Off => "off",
        }
    }
}

impl SongOptions {
    pub fn new(engine: SongEngine, style: &str) -> Self {
        SongOptions {
            engine,
            server: InlineText::default(),
            style: InlineText::new(style),
            lyrics: InlineText::default(),
            instrumental: false,
            score: InlineText::default(),
            plan: ScorePlan::Full,
            seconds: DEFAULT_SECONDS,
            seed: 0,
            bpm: 0,
            key: InlineText::default(),
            planner: true,
        }
    }

    /// The server address, without a trailing `/`.
    pub fn server_url(&self) -> String {
        let s = self.server.as_str().trim().trim_end_matches('/');
        if s.is_empty() {
            self.engine.default_server().to_string()
        } else if s.contains("://") {
            s.to_string()
        } else {
            format!("http://{s}")
        }
    }

    /// The settings, checked.
    pub fn validated(self) -> Result<Self> {
        if self.style.is_empty() && self.lyrics.is_empty() && self.score.is_empty() {
            return Err(MlError::BadInput("describe the song (style), or give lyrics or a score".into()));
        }
        if !self.seconds.is_finite() || !(10.0..=MAX_SECONDS).contains(&self.seconds) {
            return Err(MlError::BadInput(format!("`seconds` must be between 10 and {MAX_SECONDS}")));
        }
        if self.bpm != 0 && !(30..=300).contains(&self.bpm) {
            return Err(MlError::BadInput("`bpm` must be between 30 and 300 (or 0: the model picks)".into()));
        }
        let url = self.server_url();
        if !(url.starts_with("http://") || url.starts_with("https://")) || url.contains(char::is_whitespace) {
            return Err(MlError::BadInput(format!("`server` must be an http address, not `{url}`")));
        }
        Ok(self)
    }

    /// The JSON request for YuE2's `/synth`, or ACE-Step's `/lm` (and `/synth`).
    pub fn request(&self) -> serde_json::Value {
        let seed = if self.seed == 0 { -1 } else { i64::try_from(self.seed & 0x7FFF_FFFF_FFFF_FFFF).unwrap_or(-1) };
        match self.engine {
            SongEngine::Yue2 => {
                let mut r = serde_json::json!({
                    "style": self.style.as_str(),
                    "lyrics": if self.instrumental { "" } else { self.lyrics.as_str() },
                    "cot": self.plan.id(),
                    "duration": self.seconds,
                    "seed": seed,
                    "lm_seed": seed,
                    "output_format": "wav24",
                });
                if !self.score.is_empty() {
                    r["abc"] = serde_json::json!(self.score.as_str());
                }
                r
            }
            SongEngine::AceStep => {
                let lyrics = if self.instrumental { "[Instrumental]" } else { self.lyrics.as_str() };
                serde_json::json!({
                    "caption": self.style.as_str(),
                    "lyrics": lyrics,
                    "duration": self.seconds,
                    "bpm": self.bpm,
                    "keyscale": self.key.as_str(),
                    "seed": seed,
                    "lm_mode": "generate",
                    "output_format": "wav24",
                })
            }
        }
    }
}

/// What a song job made.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Song {
    /// The audio file as the server sent it (WAV).
    pub audio: Vec<u8>,
    /// The request that reproduces the track (YuE2's replay request with its seeds and score;
    /// ACE-Step's planned request).
    pub request: serde_json::Value,
    /// YuE2: the score the song follows (ABC).
    pub score: Option<String>,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(120))
        .user_agent(concat!("FreeDAWm/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn unreachable(engine: SongEngine, url: &str, why: &str) -> MlError {
    let bin = match engine {
        SongEngine::Yue2 => "yue-server (yue2.cpp)",
        SongEngine::AceStep => "ace-server (acestep.cpp)",
    };
    MlError::NotInstalled(format!(
        "no {} server at {url} ({why}): start {bin} there, or leave the server address empty to use the engine FreeDAWm installs (Setup › ML Models)",
        engine.label()
    ))
}

/// The error a server's JSON body (`{"error": ...}`) names, else the status.
fn server_error(code: u16, resp: ureq::Response) -> MlError {
    let mut body = String::new();
    let _ = resp.into_reader().take(64 * 1024).read_to_string(&mut body);
    let msg = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("error").and_then(serde_json::Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.chars().take(200).collect());
    if code == 400 {
        MlError::BadInput(format!("the song server refused the request: {msg}"))
    } else {
        MlError::Runtime(format!("the song server answered {code}: {msg}"))
    }
}

/// The most a result may weigh (10 minutes of 48 kHz stereo f32 is 230 MB; latents and JSON on top).
const MAX_RESULT_BYTES: u64 = 512 * 1024 * 1024;

struct Client {
    engine: SongEngine,
    url: String,
    agent: ureq::Agent,
}

impl Client {
    fn call(&self, req: ureq::Request, body: Option<&serde_json::Value>) -> Result<ureq::Response> {
        let r = match body {
            Some(b) => req.send_json(b.clone()),
            None => req.call(),
        };
        match r {
            Ok(r) => Ok(r),
            Err(ureq::Error::Status(code, resp)) => Err(server_error(code, resp)),
            Err(ureq::Error::Transport(t)) => Err(unreachable(self.engine, &self.url, &t.to_string())),
        }
    }

    fn json(resp: ureq::Response) -> Result<serde_json::Value> {
        let mut s = String::new();
        resp.into_reader()
            .take(64 * 1024 * 1024)
            .read_to_string(&mut s)
            .map_err(|e| MlError::Io(format!("reading the song server's answer: {e}")))?;
        serde_json::from_str(&s).map_err(|e| MlError::Runtime(format!("the song server's answer is not JSON: {e}")))
    }

    /// Submit to `path`; the job id.
    fn submit(&self, path: &str, body: &serde_json::Value) -> Result<String> {
        let v = Self::json(self.call(self.agent.post(&format!("{}{path}", self.url)), Some(body))?)?;
        match v.get("id") {
            Some(serde_json::Value::String(s)) => Ok(s.clone()),
            Some(serde_json::Value::Number(n)) => Ok(n.to_string()),
            _ => Err(MlError::Runtime("the song server gave no job id".into())),
        }
    }

    /// Wait for job `id` (cancelling it on the server when the person cancels), then its result:
    /// (content type, body).
    fn wait(&self, id: &str, what: &str, ctx: &Ctx, from: f32, to: f32) -> Result<(String, Vec<u8>)> {
        let started = std::time::Instant::now();
        let id_q: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
        loop {
            if ctx.cancel_flag().is_cancelled() {
                let _ = self.agent.post(&format!("{}/job?id={id_q}&cancel=1", self.url)).call();
                return Err(MlError::Cancelled);
            }
            let v = Self::json(self.call(self.agent.get(&format!("{}/job?id={id_q}", self.url)), None)?)?;
            match v.get("status").and_then(serde_json::Value::as_str).unwrap_or("") {
                "done" => break,
                "failed" => {
                    let why = v.get("error").and_then(serde_json::Value::as_str).unwrap_or("see the server's log");
                    return Err(MlError::Runtime(format!("the song server failed: {why}")));
                }
                "cancelled" => return Err(MlError::Cancelled),
                _ => {}
            }
            let secs = started.elapsed().as_secs();
            ctx.say(&format!("{what} ({}:{:02})", secs / 60, secs % 60));
            // No progress from the server: creep towards the stage's end.
            let f = 1.0 - (-(started.elapsed().as_secs_f32()) / 120.0).exp();
            ctx.report(from + (to - from) * 0.95 * f);
            std::thread::sleep(Duration::from_millis(400));
        }
        let resp = self.call(self.agent.get(&format!("{}/job?id={id_q}&result=1", self.url)), None)?;
        let ctype = resp.header("content-type").unwrap_or("").to_string();
        let mut body = Vec::new();
        resp.into_reader().take(MAX_RESULT_BYTES).read_to_end(&mut body).map_err(|e| MlError::Io(format!("reading the song: {e}")))?;
        Ok((ctype, body))
    }
}

/// One part of a `multipart/mixed` body: (content type, body).
pub fn multipart(content_type: &str, body: &[u8]) -> Vec<(String, Vec<u8>)> {
    let Some(boundary) = content_type.split(';').filter_map(|p| p.trim().strip_prefix("boundary=")).next() else { return Vec::new() };
    let boundary = boundary.trim_matches('"');
    let delim = format!("--{boundary}");
    let d = delim.as_bytes();
    let mut starts = Vec::new();
    let mut i = 0;
    while i + d.len() <= body.len() {
        if body.get(i..i + d.len()) == Some(d) && (i == 0 || body.get(i.saturating_sub(1)) == Some(&b'\n')) {
            starts.push(i);
            i += d.len();
        } else {
            i += 1;
        }
    }
    let mut parts = Vec::new();
    for w in starts.windows(2) {
        let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else { continue };
        let Some(chunk) = body.get(a + d.len()..b) else { continue };
        // Skip the line end after the delimiter; headers end at a blank line.
        let chunk = chunk.strip_prefix(b"\r\n").or_else(|| chunk.strip_prefix(b"\n")).unwrap_or(chunk);
        let split = find(chunk, b"\r\n\r\n").map(|p| (p, 4)).or_else(|| find(chunk, b"\n\n").map(|p| (p, 2)));
        let Some((h, sep)) = split else { continue };
        let headers = String::from_utf8_lossy(chunk.get(..h).unwrap_or(&[])).to_string();
        let mut data = chunk.get(h + sep..).unwrap_or(&[]);
        // The line end before the next delimiter belongs to it.
        data = data.strip_suffix(b"\r\n").or_else(|| data.strip_suffix(b"\n")).unwrap_or(data);
        let ctype = headers
            .lines()
            .filter_map(|l| l.split_once(':'))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.trim().to_ascii_lowercase())
            .unwrap_or_default();
        parts.push((ctype, data.to_vec()));
    }
    parts
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The first track of a synth result: (audio, its replay request if any).
fn first_track(ctype: &str, body: Vec<u8>) -> Result<(Vec<u8>, Option<serde_json::Value>)> {
    if ctype.starts_with("audio/") {
        return Ok((body, None));
    }
    let parts = multipart(ctype, &body);
    let audio = parts.iter().find(|(t, _)| t.starts_with("audio/")).map(|(_, b)| b.clone());
    let json = parts.iter().find(|(t, _)| t.starts_with("application/json")).and_then(|(_, b)| serde_json::from_slice(b).ok());
    let audio = audio.ok_or_else(|| MlError::Runtime("the song server's result holds no audio".into()))?;
    Ok((audio, json))
}

/// Generate a song: with no `server` address, through the engine FreeDAWm runs itself
/// ([`crate::engine_server`], installed in `models_dir` from Setup › ML Models, on `device`);
/// else through the server at that address. Waits for it (cancellable) and returns the audio.
pub fn generate(opts: &SongOptions, models_dir: &std::path::Path, device: crate::Device, ctx: &Ctx) -> Result<Song> {
    let opts = opts.validated()?;
    let _lease;
    let url = if opts.server.is_empty() {
        let model = crate::models::load(models_dir, opts.engine.id()).map_err(|e| match e {
            MlError::NotInstalled(_) => MlError::NotInstalled(format!(
                "the {} song engine is not installed: download it in Setup › ML Models (or give the address of a server you run)",
                opts.engine.label()
            )),
            other => other,
        })?;
        let lease = crate::engine_server::acquire(&model, device, ctx.cancel_flag(), &|w| ctx.say(w))?;
        let url = lease.url.clone();
        _lease = Some(lease);
        url
    } else {
        _lease = None;
        opts.server_url()
    };
    run(&opts, url, ctx)
}

fn run(opts: &SongOptions, url: String, ctx: &Ctx) -> Result<Song> {
    let client = Client { engine: opts.engine, url, agent: agent() };
    ctx.say(&format!("Connecting to {}", client.url));
    // Health first: a clear message when nothing is listening.
    let _ = Client::json(client.call(client.agent.get(&format!("{}/health", client.url)), None)?)?;
    let mut request = opts.request();
    match opts.engine {
        SongEngine::Yue2 => {
            let id = client.submit("/synth", &request)?;
            let (ctype, body) = client.wait(&id, "Composing and singing (YuE2)", ctx, 0.0, 1.0)?;
            let (audio, replay) = first_track(&ctype, body)?;
            let score =
                replay.as_ref().and_then(|r| r.get("abc")).and_then(serde_json::Value::as_str).filter(|s| !s.trim().is_empty()).map(str::to_string);
            ctx.report(1.0);
            Ok(Song { audio, request: replay.unwrap_or(request), score })
        }
        SongEngine::AceStep => {
            if opts.planner {
                let id = client.submit("/lm", &request)?;
                let (_, body) = client.wait(&id, "Planning (ACE-Step 5 Hz LM)", ctx, 0.0, 0.4)?;
                let planned: serde_json::Value =
                    serde_json::from_slice(&body).map_err(|e| MlError::Runtime(format!("the planner's answer is not JSON: {e}")))?;
                let first = match planned {
                    serde_json::Value::Array(mut a) if !a.is_empty() => a.swap_remove(0),
                    v @ serde_json::Value::Object(_) => v,
                    _ => return Err(MlError::Runtime("the planner returned no request".into())),
                };
                request = first;
                request["output_format"] = serde_json::json!("wav24");
            }
            let id = client.submit("/synth", &request)?;
            let (ctype, body) = client.wait(&id, "Rendering (ACE-Step DiT and VAE)", ctx, if opts.planner { 0.4 } else { 0.0 }, 1.0)?;
            let (audio, _) = first_track(&ctype, body)?;
            ctx.report(1.0);
            Ok(Song { audio, request, score: None })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_text_is_bounded_and_clean() {
        let t = InlineText::<8>::new("  héllo wörld \r\n");
        assert_eq!(t.as_str(), "héllo w");
        assert!(InlineText::<8>::would_cut("123456789"));
        assert_eq!(InlineText::<16>::new("a\r\nb\0c").as_str(), "a\nbc");
        assert!(InlineText::<4>::default().is_empty());
    }

    #[test]
    fn requests_follow_each_server() {
        let mut o = SongOptions::new(SongEngine::Yue2, "warm piano pop");
        o.lyrics = InlineText::new("[Verse]\nla la");
        o.seed = 42;
        let r = o.request();
        assert_eq!(
            (r["style"].as_str(), r["cot"].as_str(), r["seed"].as_i64(), r["lm_seed"].as_i64()),
            (Some("warm piano pop"), Some("full"), Some(42), Some(42))
        );
        assert!(r.get("abc").is_none());
        o.score = InlineText::new("X:1\nK:C\nC");
        o.instrumental = true;
        assert_eq!(o.request()["abc"], "X:1\nK:C\nC");
        assert_eq!(o.request()["lyrics"], "");
        let mut a = SongOptions::new(SongEngine::AceStep, "lofi");
        a.instrumental = true;
        a.bpm = 90;
        let r = a.request();
        assert_eq!(
            (r["caption"].as_str(), r["lyrics"].as_str(), r["bpm"].as_u64(), r["seed"].as_i64()),
            (Some("lofi"), Some("[Instrumental]"), Some(90), Some(-1))
        );
        assert_eq!(a.server_url(), "http://127.0.0.1:8080");
        a.server = InlineText::new("localhost:9000/");
        assert_eq!(a.server_url(), "http://localhost:9000");
    }

    #[test]
    fn options_are_checked() {
        assert!(SongOptions::new(SongEngine::Yue2, "pop").validated().is_ok());
        assert!(SongOptions::new(SongEngine::Yue2, "").validated().is_err());
        assert!(SongOptions { seconds: 5.0, ..SongOptions::new(SongEngine::Yue2, "pop") }.validated().is_err());
        assert!(SongOptions { bpm: 999, ..SongOptions::new(SongEngine::AceStep, "pop") }.validated().is_err());
        assert!(SongOptions { server: InlineText::new("ftp://x"), ..SongOptions::new(SongEngine::AceStep, "pop") }.validated().is_err());
        assert_eq!(SongEngine::parse("ACE-Step"), Some(SongEngine::AceStep));
        assert_eq!(SongEngine::parse("YuE2"), Some(SongEngine::Yue2));
        assert_eq!(ScorePlan::parse("melody"), Some(ScorePlan::Melody));
    }

    #[test]
    fn multipart_parts_come_apart() {
        let body = b"--b0\r\nContent-Type: application/json\r\n\r\n{\"abc\":\"X:1\"}\r\n--b0\r\nContent-Type: audio/wav\r\n\r\nRIFF\r\n\r\nxx\r\n--b0\r\nContent-Type: application/octet-stream\r\n\r\n\x00\x01\r\n--b0--\r\n";
        let parts = multipart("multipart/mixed; boundary=b0", body);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].0, "application/json");
        assert_eq!(parts[1].1, b"RIFF\r\n\r\nxx");
        let (audio, json) = first_track("multipart/mixed; boundary=\"b0\"", body.to_vec()).unwrap();
        assert_eq!(audio, b"RIFF\r\n\r\nxx");
        assert_eq!(json.unwrap()["abc"], "X:1");
        assert!(multipart("multipart/mixed", body).is_empty());
        assert!(first_track("multipart/mixed; boundary=zz", body.to_vec()).is_err());
    }

    #[test]
    fn nothing_listening_is_a_clear_refusal() {
        // A port nothing listens on (bound, then closed).
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let o = SongOptions { server: InlineText::new(&format!("127.0.0.1:{port}")), ..SongOptions::new(SongEngine::AceStep, "pop") };
        let e = generate(&o, std::path::Path::new("/nowhere"), crate::Device::Auto, &Ctx::default()).unwrap_err();
        assert!(matches!(e, MlError::NotInstalled(ref m) if m.contains("ace-server")), "{e:?}");
        // No address and no engine installed: download it.
        let o = SongOptions::new(SongEngine::Yue2, "pop");
        let empty = std::env::temp_dir().join(format!("freedawm-no-engines-{}", std::process::id()));
        let e = generate(&o, &empty, crate::Device::Auto, &Ctx::default()).unwrap_err();
        assert!(matches!(e, MlError::NotInstalled(ref m) if m.contains("Setup › ML Models")), "{e:?}");
    }
}
