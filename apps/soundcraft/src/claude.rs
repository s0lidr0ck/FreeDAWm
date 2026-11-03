//! Scout's call to Claude (the Anthropic Messages API, streamed) and the owner's API key.
//!
//! The key lives in its own file in the settings folder (`scout-key`), written atomically, never
//! in the session, never in `UiState` (which the control channel can read), never logged, and
//! only ever shown masked. The request runs on its own thread; every failure (no key, no network,
//! a refused key, rate limiting, a cut or malformed stream) becomes one plain `Error` event.

use soundcraft_ui_egui::Services;
use soundcraft_ui_egui::scout::ask::{ClaudeEvent, ClaudeRequest};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
const VERSION: &str = "2023-06-01";
/// The largest answer stream read (bytes): far beyond any real answer.
const MAX_STREAM: u64 = 16 * 1024 * 1024;
/// The whole request, connect to last byte.
const DEADLINE: Duration = Duration::from_secs(300);

pub const NO_KEY: &str = "Add your Anthropic API key (Scout ▸ Settings) or use Copy";
pub const REJECTED: &str = "The API key was rejected";
pub const RATE_LIMITED: &str = "Rate limited; try again shortly";
pub const CUT: &str = "The answer was cut off (the connection closed early)";
pub const MALFORMED: &str = "Claude's answer stream couldn't be read";

/// `<settings>/scout-key`.
pub fn key_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("scout-key"))
}

/// The stored key, if any.
pub fn load_key(path: &Path) -> Option<String> {
    let mut s = String::new();
    std::fs::File::open(path).ok()?.take(4096).read_to_string(&mut s).ok()?;
    let k = s.trim();
    (!k.is_empty()).then(|| k.to_string())
}

/// Stores `key` (atomically, as the settings are saved) or, with `None`, removes it.
pub fn save_key(path: &Path, key: Option<&str>) -> Result<(), String> {
    use std::io::Write as _;
    let Some(key) = key.map(str::trim) else {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("The API key could not be removed: {}", e.kind())),
        };
    };
    if key.is_empty() || key.len() > 512 || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("That doesn't look like an API key".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("The API key could not be saved: {}", e.kind()))?;
    }
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let temporary = path.with_extension(format!("{}.{nonce}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let written = (|| -> std::io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(key.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    // The error kind only: never anything that could carry the key.
    written.map_err(|e| format!("The API key could not be saved: {}", e.kind()))
}

/// "sk-ant-…a1b2": enough to recognise the key, never enough to use it.
pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 16 {
        return format!("…{}", chars.iter().skip(chars.len().saturating_sub(2)).collect::<String>());
    }
    let head: String = chars.iter().take(7).collect();
    let tail: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{head}…{tail}")
}

/// Wires Scout's services to the key at `key_path` (none: no settings folder, so no key).
pub fn install(services: &mut Services, key_path: Option<PathBuf>) {
    let (p1, p2, p3) = (key_path.clone(), key_path.clone(), key_path);
    services.ask_claude =
        Some(Arc::new(move |req: ClaudeRequest, mut on_event: Box<dyn FnMut(ClaudeEvent) + Send>| match p1.as_deref().and_then(load_key) {
            Some(key) => ask(API_URL.to_string(), req, key, on_event),
            None => on_event(ClaudeEvent::Error(NO_KEY.into())),
        }));
    services.api_key_status = Some(Arc::new(move || p2.as_deref().and_then(load_key).map(|k| mask(&k))));
    services.set_api_key = Some(Arc::new(move |key: Option<String>| match &p3 {
        Some(p) => save_key(p, key.as_deref()),
        None => Err("There's no settings folder to keep the API key in".into()),
    }));
}

/// Sends `req` to `url` on its own thread, reporting the answer as it streams.
pub fn ask(url: String, req: ClaudeRequest, key: String, on_event: Box<dyn FnMut(ClaudeEvent) + Send>) {
    // Shared so a thread that can't start can still say so.
    let shared = Arc::new(Mutex::new(on_event));
    let inner = shared.clone();
    let spawned = std::thread::Builder::new().name("scout-claude".into()).spawn(move || {
        let mut emit = |ev: ClaudeEvent| {
            if let Ok(mut f) = inner.lock() {
                f(ev);
            }
        };
        ask_blocking(&url, &req, &key, &mut emit);
    });
    if spawned.is_err()
        && let Ok(mut f) = shared.lock()
    {
        f(ClaudeEvent::Error("Scout couldn't start the request".into()));
    }
}

/// The request itself, on the calling thread: ends with exactly one `Done` or `Error`.
pub fn ask_blocking(url: &str, req: &ClaudeRequest, key: &str, emit: &mut dyn FnMut(ClaudeEvent)) {
    // No redirects: the key goes to the API host and nowhere else. The overall deadline ends a
    // request that trickles forever.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .timeout(DEADLINE)
        .redirects(0)
        .build();
    let resp = agent
        .post(url)
        .set("x-api-key", key)
        .set("anthropic-version", VERSION)
        .set("content-type", "application/json")
        .set("accept", "text/event-stream")
        .send_json(req.body());
    let resp = match resp {
        // A redirect (not followed) or any other non-success answer.
        Ok(r) if !(200..300).contains(&r.status()) => {
            emit(ClaudeEvent::Error(format!("Claude request failed (status {})", r.status())));
            return;
        }
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            emit(ClaudeEvent::Error(status_message(code, r)));
            return;
        }
        Err(ureq::Error::Transport(t)) => {
            emit(ClaudeEvent::Error(format!("Couldn't reach Claude ({}); check the network", t.kind())));
            return;
        }
    };
    match read_stream(BufReader::new(resp.into_reader().take(MAX_STREAM)), emit) {
        Ok(()) => emit(ClaudeEvent::Done),
        Err(e) => emit(ClaudeEvent::Error(e)),
    }
}

/// API text shown to the user: at most 300 characters, with anything that looks like a key
/// (`sk-ant-…`) cut down to its prefix.
pub fn echo(text: &str) -> String {
    let text: String = text.chars().take(300).collect();
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("sk-ant-") {
        out.push_str(rest.get(..at).unwrap_or(""));
        out.push_str("sk-ant-…");
        let tail = rest.get(at + 7..).unwrap_or("");
        let end = tail.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(tail.len());
        rest = tail.get(end..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// The plain message for an HTTP error status, with the API's own explanation when it gives one.
fn status_message(code: u16, r: ureq::Response) -> String {
    let base = match code {
        401 => return REJECTED.into(),
        429 => return RATE_LIMITED.into(),
        _ => format!("Claude request failed (status {code})"),
    };
    let mut body = String::new();
    let _ = r.into_reader().take(64 * 1024).read_to_string(&mut body);
    let detail = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| v["error"]["message"].as_str().map(echo));
    match detail {
        Some(d) if !d.is_empty() => format!("{base}: {d}"),
        _ => base,
    }
}

/// Reads the server-sent events: text deltas out as `Text`; `Ok` at `message_stop`. A stream that
/// ends first, an `error` event or unreadable data is an `Err` with a plain message.
pub fn read_stream(reader: impl BufRead, emit: &mut dyn FnMut(ClaudeEvent)) -> Result<(), String> {
    let mut data = String::new();
    for line in reader.lines() {
        let line = line.map_err(|e| match e.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "Claude stopped answering (timed out)".to_string(),
            std::io::ErrorKind::InvalidData => MALFORMED.to_string(),
            _ => CUT.to_string(),
        })?;
        if line.is_empty() {
            if dispatch(&std::mem::take(&mut data), emit)? {
                return Ok(());
            }
        } else if let Some(d) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(d.trim_start());
        }
        // `event:` lines repeat the data's `type`; comments and other fields are ignored.
    }
    // The last event may end at the close without its blank line; a cut-off one won't parse.
    if !data.is_empty() && dispatch(&data, emit).unwrap_or(false) {
        return Ok(());
    }
    Err(CUT.into())
}

/// One event's data; `Ok(true)` at the end of the message.
fn dispatch(data: &str, emit: &mut dyn FnMut(ClaudeEvent)) -> Result<bool, String> {
    if data.is_empty() {
        return Ok(false);
    }
    let v: serde_json::Value = serde_json::from_str(data).map_err(|_| MALFORMED.to_string())?;
    match v["type"].as_str().unwrap_or("") {
        "content_block_delta" => {
            if v["delta"]["type"] == "text_delta"
                && let Some(t) = v["delta"]["text"].as_str()
            {
                emit(ClaudeEvent::Text(t.to_string()));
            }
            Ok(false)
        }
        "message_stop" => Ok(true),
        "error" => {
            let m = v["error"]["message"].as_str().unwrap_or("unknown error");
            Err(format!("Claude reported an error: {}", echo(m)))
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_ui_egui::scout::ask::{Role, SYSTEM};
    use std::io::Write as _;
    use std::net::TcpListener;

    fn req() -> ClaudeRequest {
        ClaudeRequest { model: "claude-opus-5-5".into(), system: SYSTEM.into(), messages: vec![(Role::User, "Is it harsh?".into())], max_tokens: 64 }
    }

    /// A fake key of the real shape, split so secret scanners do not take the source for a key.
    const KEY: &str = concat!("sk-", "ant-", "test-", "NOT-A-REAL-KEY-", "a1b2");

    /// A one-shot HTTP server: reads the request, answers with `response`, closes. Returns the
    /// URL and a handle giving back the raw request.
    fn stub(response: String) -> (String, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/messages", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 8192];
            // Headers, then the body by Content-Length.
            loop {
                let n = s.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if got.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            s.write_all(response.as_bytes()).unwrap();
            let _ = s.flush();
            String::from_utf8_lossy(&got).to_string()
        });
        (url, h)
    }

    fn sse(events: &[&str]) -> String {
        let body: String = events.iter().map(|e| format!("event: x\ndata: {e}\n\n")).collect();
        format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}")
    }

    fn run(url: &str) -> Vec<ClaudeEvent> {
        let mut out = Vec::new();
        ask_blocking(url, &req(), KEY, &mut |e| out.push(e));
        out
    }

    #[test]
    fn streams_text_and_finishes() {
        let (url, h) = stub(sse(&[
            r#"{"type":"message_start","message":{"id":"m"}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Harsh "}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"at 3 kHz."}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            r#"{"type":"message_stop"}"#,
        ]));
        let ev = run(&url);
        assert_eq!(ev, vec![ClaudeEvent::Text("Harsh ".into()), ClaudeEvent::Text("at 3 kHz.".into()), ClaudeEvent::Done]);
        let sent = h.join().unwrap();
        let lower = sent.to_ascii_lowercase();
        assert!(sent.starts_with("POST /v1/messages"));
        assert!(lower.contains(&format!("x-api-key: {}", KEY.to_ascii_lowercase())));
        assert!(lower.contains("anthropic-version: 2023-06-01"));
        assert!(lower.contains("content-type: application/json"));
        let body: serde_json::Value = serde_json::from_str(&sent[sent.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["system"], SYSTEM);
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "Is it harsh?");
    }

    fn status(code: u16, body: &str) -> String {
        format!("HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    fn only_error(ev: &[ClaudeEvent]) -> String {
        match ev {
            [ClaudeEvent::Error(e)] => e.clone(),
            other => panic!("expected one error, got {other:?}"),
        }
    }

    #[test]
    fn errors_become_messages_in_the_conversation() {
        let (url, _h) = stub(status(401, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#));
        assert_eq!(only_error(&run(&url)), REJECTED);
        let (url, _h) = stub(status(429, "{}"));
        assert_eq!(only_error(&run(&url)), RATE_LIMITED);
        let (url, _h) = stub(status(500, "oops"));
        assert_eq!(only_error(&run(&url)), "Claude request failed (status 500)");
        let (url, _h) = stub(status(400, r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long"}}"#));
        assert_eq!(only_error(&run(&url)), "Claude request failed (status 400): prompt is too long");
        // Connection refused: a port nobody listens on.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let e = only_error(&run(&format!("http://127.0.0.1:{port}/v1/messages")));
        assert!(e.starts_with("Couldn't reach Claude"), "{e}");
        // The stream cut mid-event: text so far, then the error, never Done.
        let mut cut = sse(&[r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Half"}}"#]);
        cut.push_str("event: content_block_delta\ndata: {\"type\":\"content_block_del");
        let (url, _h) = stub(cut);
        let ev = run(&url);
        assert_eq!(ev, vec![ClaudeEvent::Text("Half".into()), ClaudeEvent::Error(CUT.into())]);
        // An error event mid-stream, and data that isn't JSON.
        let (url, _h) = stub(sse(&[r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#]));
        assert_eq!(only_error(&run(&url)), "Claude reported an error: Overloaded");
        let (url, _h) = stub(sse(&["not json"]));
        assert_eq!(only_error(&run(&url)), MALFORMED);
        // The thread version ends with exactly one Error too.
        let (url, _h) = stub(status(401, "{}"));
        let (tx, rx) = std::sync::mpsc::channel();
        ask(url, req(), KEY.into(), Box::new(move |e| tx.send(e).unwrap()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), ClaudeEvent::Error(REJECTED.into()));
        // No key: the message, and no request.
        let mut services = Services::default();
        install(&mut services, Some(std::env::temp_dir().join(format!("sc-no-key-{}", std::process::id())).join("scout-key")));
        let (tx, rx) = std::sync::mpsc::channel();
        (services.ask_claude.unwrap())(req(), Box::new(move |e| tx.send(e).unwrap()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), ClaudeEvent::Error(NO_KEY.into()));
    }

    #[test]
    fn key_is_never_in_the_session_or_ui_json() {
        let dir = std::env::temp_dir().join(format!("soundcraft-scout-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("scout-key");
        let mut services = Services::default();
        install(&mut services, Some(path.clone()));
        assert_eq!((services.api_key_status.as_ref().unwrap())(), None);
        assert!((services.set_api_key.as_ref().unwrap())(Some("has space".into())).is_err());
        (services.set_api_key.as_ref().unwrap())(Some(format!("  {KEY}\n"))).unwrap();
        assert_eq!(load_key(&path).as_deref(), Some(KEY), "kept in its own file, trimmed");
        let masked = (services.api_key_status.as_ref().unwrap())().unwrap();
        assert_eq!(masked, "sk-ant-…a1b2");
        // The key's middle: never shown, stored only in its own file.
        let middle = "NOT-A-REAL-KEY";
        let mut app = soundcraft_ui_egui::SoundApp::new(soundcraft_engine::Engine::default(), None, services);
        app.scout.question = "q".into();
        let ui_json = serde_json::to_string(&app.ui).unwrap();
        let session_json = serde_json::to_string(app.engine.session()).unwrap();
        assert!(!ui_json.contains(middle) && !session_json.contains(middle));
        assert!(ui_json.contains("\"scout_model\":\"claude-opus-5-5\""), "the model is a setting; the key is not");
        assert!(app.scout.key_draft.is_empty());
        // Errors never carry it either, whatever the API echoes back.
        for code in [401, 400, 403] {
            let (url, _h) = stub(status(code, &format!("{{\"error\":{{\"message\":\"bad key {KEY} given\"}}}}")));
            let e = only_error(&run(&url));
            assert!(!e.contains(middle), "{code}: {e}");
            if code != 401 {
                assert!(e.ends_with("bad key sk-ant-… given"), "{code}: {e}");
            }
        }
        let (url, _h) = stub(sse(&[&format!("{{\"type\":\"error\",\"error\":{{\"message\":\"{KEY}\"}}}}")]));
        assert!(!only_error(&run(&url)).contains(middle));
        // Removing it.
        (app.services.set_api_key.as_ref().unwrap())(None).unwrap();
        assert_eq!((app.services.api_key_status.as_ref().unwrap())(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn redirects_are_not_followed() {
        // The key must never be sent on to another host.
        let (url, h) = stub(
            "HTTP/1.1 307 Moved
Location: http://127.0.0.1:9/steal
Content-Length: 0
Connection: close

"
            .into(),
        );
        let ev = run(&url);
        assert_eq!(ev, vec![ClaudeEvent::Error("Claude request failed (status 307)".into())]);
        let _ = h.join();
    }

    #[test]
    fn short_keys_are_masked_too() {
        assert_eq!(mask("abc"), "…bc");
        assert!(!mask(concat!("sk-", "ant-", "0123456789abcdef")).contains("0123456789ab"));
    }
}
