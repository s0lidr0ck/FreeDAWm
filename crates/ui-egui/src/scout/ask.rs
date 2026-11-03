//! Asking Claude from the Scout panel: the request (system prompt, the conversation so far, the
//! question with a fresh package of the ticked captures), the answer streaming into the panel,
//! and the finished answer stored with `scout.answer` (its guides parsed, bad ones noted).
//!
//! The network call is the host's (`Services::ask_claude`, native only): this crate stays
//! wasm-clean. The API key never passes through here; the host reads it from its own settings
//! file. Errors (no key, no network, a refused key, a cut stream) become plain messages in the
//! conversation, and the question goes back in the box.

use super::{guides, package};
use crate::SoundApp;
use serde_json::{Value, json};
use soundcraft_model::{ScoutTurn, Session};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

/// Claude's model unless the user picks another (Scout ▸ Settings).
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
pub const SYSTEM: &str = "You are Scout, an in-DAW mixing assistant. Follow ai_response_instructions in the package.";
pub const MAX_TOKENS: u32 = 8192;
/// Earlier turns sent with a follow-up (the newest ones; older ones are left out).
pub const CONTEXT_TURNS: usize = 8;
/// The hint where there is no network call (the web build).
pub const WEB_HINT: &str = "Use Copy on the web build";
pub const EMPTY: &str = "Claude sent an empty answer";
pub const STOPPED: &str = "The answer stopped before it finished";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// One Messages API request.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<(Role, String)>,
    pub max_tokens: u32,
}

impl ClaudeRequest {
    /// The JSON body for `POST /v1/messages`, streamed.
    pub fn body(&self) -> Value {
        let messages: Vec<Value> = self.messages.iter().map(|(r, text)| json!({"role": r.as_str(), "content": text})).collect();
        json!({"model": self.model, "max_tokens": self.max_tokens, "system": self.system, "messages": messages, "stream": true})
    }
}

/// What the host reports back while the answer streams.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeEvent {
    Text(String),
    Done,
    Error(String),
}

/// `Services::ask_claude`: sends a request and reports each event, from any thread.
pub type AskClaude = Arc<dyn Fn(ClaudeRequest, Box<dyn FnMut(ClaudeEvent) + Send>) + Send + Sync>;
/// `Services::api_key_status`: the key masked ("sk-ant-…a1b2"), or `None` without one.
pub type KeyStatus = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// `Services::set_api_key`: stores (or with `None` removes) the key in the host's settings.
pub type SetKey = Arc<dyn Fn(Option<String>) -> Result<(), String> + Send + Sync>;

/// A question out with Claude.
pub struct Asking {
    pub question: String,
    pub captures: Vec<u32>,
    pub model: String,
    /// The answer so far.
    pub text: String,
    /// Set when it failed: the message shown in the conversation.
    pub error: Option<String>,
    /// The track names of the package sent (what guides may name).
    known: Vec<String>,
    /// The session it was asked in (`Engine::loads`): an answer never lands in another one.
    loads: u64,
    rx: Option<Receiver<ClaudeEvent>>,
}

impl Asking {
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }
}

/// The messages for `question`: the earlier answered turns as user/assistant pairs (the newest
/// [`CONTEXT_TURNS`]), then the question with its package.
pub fn messages(s: &Session, prompt: String) -> Vec<(Role, String)> {
    let answered: Vec<&ScoutTurn> = s.scout.turns.iter().filter(|t| t.error.is_none() && !t.answer.trim().is_empty()).collect();
    let skip = answered.len().saturating_sub(CONTEXT_TURNS);
    let mut out = Vec::new();
    for t in answered.into_iter().skip(skip) {
        let q = if t.question.trim().is_empty() { "(question)" } else { t.question.as_str() };
        out.push((Role::User, q.to_string()));
        out.push((Role::Assistant, t.answer.clone()));
    }
    out.push((Role::User, prompt));
    out
}

/// The strips' track names in a package (`track.name` / `tracks[].name`).
fn package_tracks(pkg: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let one = pkg.get("track").into_iter();
    let many = pkg.get("tracks").and_then(Value::as_array).into_iter().flatten();
    for t in one.chain(many) {
        if let Some(n) = t.get("name").and_then(Value::as_str)
            && !names.iter().any(|x| x == n)
        {
            names.push(n.to_string());
        }
    }
    names
}

/// Whether Send can work here: a host network call (never on the web build).
pub fn can_send(app: &SoundApp) -> bool {
    app.services.ask_claude.is_some()
}

/// The masked key, read from the host once and kept until Settings saves or removes it (no
/// per-frame disk reads).
pub fn key_status(app: &mut SoundApp) -> Option<String> {
    if app.scout.key_status.is_none() {
        app.scout.key_status = Some(app.services.api_key_status.as_ref().and_then(|f| f()));
    }
    app.scout.key_status.clone().flatten()
}

/// Whether a key is set. Without a status service the host decides (it reports a missing key).
pub fn has_key(app: &mut SoundApp) -> bool {
    app.services.api_key_status.is_none() || key_status(app).is_some()
}

/// Cancel: stops waiting for the answer in flight (the receiver is dropped; whatever the host
/// still sends goes nowhere) and puts the question back in the box.
pub fn cancel(app: &mut SoundApp) {
    if let Some(a) = app.scout.asking.take()
        && app.scout.question.trim().is_empty()
    {
        app.scout.question = a.question;
    }
}

/// The session was replaced: a question out with Claude belonged to the old one.
pub(crate) fn session_replaced(app: &mut SoundApp) {
    app.scout.asking = None;
}

/// `ui.scout.send {question?, captures?}`: sends the question with the package of the captures
/// (default: the typed question and the ticked captures).
pub fn send(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let Some(ask) = app.services.ask_claude.clone() else {
        return Err(WEB_HINT.into());
    };
    if app.scout.asking.as_ref().is_some_and(Asking::busy) {
        return Err("Scout is still waiting for the last answer".into());
    }
    if !has_key(app) {
        app.scout.settings_open = true;
        app.scout.message = Some(super::panel::NO_KEY.into());
        return Err(super::panel::NO_KEY.into());
    }
    let (pkg, question) = super::package_for(app, p)?;
    if question.trim().is_empty() {
        return Err("Type a question first".into());
    }
    let captures = super::captures_param(app, p);
    let model = if app.ui.scout_model.trim().is_empty() { DEFAULT_MODEL.to_string() } else { app.ui.scout_model.trim().to_string() };
    let req = ClaudeRequest {
        model: model.clone(),
        system: SYSTEM.into(),
        messages: messages(app.engine.session(), package::prompt_text(&pkg, &question)),
        max_tokens: MAX_TOKENS,
    };
    let (tx, rx) = channel();
    let ctx = app.scout.ctx.clone();
    ask(
        req,
        Box::new(move |ev| {
            let _ = tx.send(ev);
            if let Some(c) = &ctx {
                c.request_repaint();
            }
        }),
    );
    app.scout.asking = Some(Asking {
        question: question.clone(),
        captures,
        model,
        text: String::new(),
        error: None,
        known: package_tracks(&pkg),
        loads: app.engine.loads(),
        rx: Some(rx),
    });
    app.scout.message = None;
    if app.scout.question.trim() == question.trim() {
        app.scout.question.clear();
    }
    Ok(json!({"sending": true}))
}

/// Each frame: takes what arrived; a finished answer is stored as a turn.
pub(crate) fn poll(app: &mut SoundApp) {
    let Some(a) = &mut app.scout.asking else { return };
    let Some(rx) = &a.rx else { return };
    let mut finished = false;
    loop {
        match rx.try_recv() {
            Ok(ClaudeEvent::Text(t)) => a.text.push_str(&t),
            Ok(ClaudeEvent::Done) if a.text.trim().is_empty() => {
                a.error = Some(EMPTY.into());
                break;
            }
            Ok(ClaudeEvent::Done) => {
                finished = true;
                break;
            }
            Ok(ClaudeEvent::Error(e)) => {
                a.error = Some(e);
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            // A host that dropped its callback without a last word ends the wait too.
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                a.error = Some(STOPPED.into());
                break;
            }
        }
    }
    if let Some(err) = a.error.clone() {
        // Failed: the message stays in the conversation and the question goes back in the box.
        a.rx = None;
        if app.scout.question.trim().is_empty() {
            app.scout.question = a.question.clone();
        }
        app.ui.status = err;
        return;
    }
    if !finished {
        return;
    }
    let Some(a) = app.scout.asking.take() else { return };
    if a.loads != app.engine.loads() {
        // Asked in a session since replaced: never stored in this one.
        log::info!("scout: an answer for a replaced session was dropped");
        return;
    }
    let (guides, notes) = guides::parse_guides(&a.text, &a.known);
    let turn =
        ScoutTurn { question: a.question.clone(), captures: a.captures, answer: a.text, guides, model: a.model, notes, ..ScoutTurn::default() };
    let stored =
        serde_json::to_value(&turn).map_err(|e| e.to_string()).and_then(|t| app.run("scout.answer", json!({"turn": t})).map_err(|e| e.to_string()));
    if let Err(e) = stored {
        log::warn!("scout.answer: {e}");
        app.scout.message = Some(format!("The answer couldn't be kept: {e}"));
        app.ui.status = e;
    }
}
