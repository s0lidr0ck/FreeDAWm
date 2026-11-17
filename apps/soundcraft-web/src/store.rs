//! The session kept in this browser between visits: the origin-private file system (OPFS),
//! through the small `window.freedawmStore` script in `index.html`. Nothing leaves the browser.
//!
//! - **What is kept:** the open session as `session.json` plus its audio under `Audio Files/`,
//!   each file named by its source and a fingerprint of its audio, so it is written once
//!   ([`soundcraft_engine::io::session_store_parts`]); `meta.json` keeps the session's name.
//! - **When:** a few seconds after a change, not while the transport plays or records; audio
//!   first, then the session, then files no longer used are removed.
//! - **On the next visit:** the kept session opens instead of the demo (`?demo` starts with the
//!   demo, `?empty` with an empty session, without touching what is kept).
//! - The page says so ("Kept in this browser only"), with Export (a `.fdawmz` download, as Save)
//!   and Clear (forget the kept copy). A browser may still clear site data under storage
//!   pressure or when the person clears it: Export is the copy to keep.

use soundcraft_engine::{Engine, io};
use soundcraft_model::Session;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = freedawmStore, js_name = available)]
    fn js_available() -> Result<bool, JsValue>;
    #[wasm_bindgen(catch, js_namespace = freedawmStore, js_name = put)]
    async fn js_put(path: &str, bytes: &[u8]) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_namespace = freedawmStore, js_name = get)]
    async fn js_get(path: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_namespace = freedawmStore, js_name = list)]
    async fn js_list() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_namespace = freedawmStore, js_name = remove)]
    async fn js_remove(path: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_namespace = freedawmStore, js_name = clear)]
    async fn js_clear() -> Result<JsValue, JsValue>;
}

const SESSION: &str = "session.json";
const META: &str = "meta.json";
/// Seconds after a change before it is kept (changes in between are kept together).
const SETTLE_S: f64 = 3.0;

/// A session read back from the store, waiting for the next frame to open.
pub struct Restored {
    pub name: String,
    pub json: String,
    pub files: HashMap<String, Vec<u8>>,
}

/// What the page knows about the store.
#[derive(Default)]
pub struct Store {
    /// The browser can keep files (OPFS); false: the notice says it cannot.
    pub available: bool,
    /// Files kept now (paths), and whether a write is under way.
    stored: BTreeSet<String>,
    busy: bool,
    /// The session last kept (pointer compared: any change makes a new one), and when the
    /// session last changed (seconds).
    kept: Option<Arc<Session>>,
    changed_at: Option<f64>,
    /// Something to say once (the shell moves it to the status line).
    pub said: Option<String>,
    /// A write failed: say why on the notice.
    pub failed: Option<String>,
    pub restored: Option<Restored>,
}

pub type Shared = Rc<RefCell<Store>>;

impl Store {
    /// Something will happen without a user event (a write, a read, a change settling).
    pub fn busy_or_waiting(&self) -> bool {
        self.busy || self.changed_at.is_some() || self.restored.is_some()
    }
}

fn now_s() -> f64 {
    js_sys::Date::now() / 1000.0
}

fn js_text(e: &JsValue) -> String {
    e.as_string().or_else(|| js_sys::JSON::stringify(e).ok().and_then(|s| s.as_string())).unwrap_or_else(|| "unknown error".into())
}

async fn get_bytes(path: &str) -> Option<Vec<u8>> {
    let v = js_get(path).await.ok()?;
    if v.is_null() || v.is_undefined() {
        return None;
    }
    Some(js_sys::Uint8Array::new(&v).to_vec())
}

async fn list() -> Vec<String> {
    match js_list().await {
        Ok(v) => js_sys::Array::from(&v).iter().filter_map(|x| x.as_string()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Starts the store: whether the browser has one, and (unless `restore` is false) reads back a
/// kept session for the next frame to open.
pub fn start(store: &Shared, restore: bool, repaint: impl Fn() + 'static) {
    let available = js_available().unwrap_or(false);
    store.borrow_mut().available = available;
    if !available {
        return;
    }
    let store = Rc::clone(store);
    wasm_bindgen_futures::spawn_local(async move {
        store.borrow_mut().busy = true;
        let files = list().await;
        let mut restored = None;
        if restore && let Some(json) = get_bytes(SESSION).await.and_then(|b| String::from_utf8(b).ok()) {
            let name = get_bytes(META)
                .await
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v["name"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("Untitled.{}", io::BUNDLE_EXTENSION));
            let mut audio = HashMap::new();
            for f in files.iter().filter(|f| f.starts_with("Audio Files/")) {
                if let Some(b) = get_bytes(f).await {
                    audio.insert(f.clone(), b);
                }
            }
            restored = Some(Restored { name, json, files: audio });
        }
        let mut s = store.borrow_mut();
        s.stored = files.into_iter().collect();
        s.restored = restored;
        s.busy = false;
        drop(s);
        repaint();
    });
}

/// The session open now counts as kept (nothing to write until it changes).
pub fn mark_kept(store: &Shared, e: &Engine) {
    store.borrow_mut().kept = Some(e.session_arc());
}

/// Opens a session read back from the store; the store then counts it as kept.
pub fn open_restored(store: &Shared, e: &mut Engine) -> Option<String> {
    let r = store.borrow_mut().restored.take()?;
    let said = match io::open_session_parts(e, &r.name, &r.json, r.files) {
        Ok(missing) if missing.is_empty() => format!("Opened {}, kept in this browser since your last visit", r.name),
        Ok(missing) => format!("Opened {}, kept in this browser ({} missing media)", r.name, missing.len()),
        Err(err) => format!("The session kept in this browser could not be opened: {err}"),
    };
    let mut s = store.borrow_mut();
    s.kept = Some(e.session_arc());
    s.changed_at = None;
    Some(said)
}

/// Once a frame: keeps the session a few seconds after it changed, unless the transport rolls
/// (`busy_audio`) or a write is under way.
pub fn tick(store: &Shared, e: &Engine, busy_audio: bool) {
    let now = now_s();
    let cur = e.session_arc();
    let parts = {
        let mut s = store.borrow_mut();
        if !s.available || s.busy || s.restored.is_some() {
            return;
        }
        if s.kept.as_ref().is_some_and(|k| Arc::ptr_eq(k, &cur)) {
            s.changed_at = None;
            return;
        }
        let since = *s.changed_at.get_or_insert(now);
        if now - since < SETTLE_S || busy_audio {
            return;
        }
        match io::session_store_parts(&cur, &s.stored) {
            Ok(p) => {
                s.busy = true;
                s.kept = Some(Arc::clone(&cur));
                s.changed_at = None;
                p
            }
            Err(err) => {
                s.failed = Some(err.to_string());
                s.kept = Some(Arc::clone(&cur));
                return;
            }
        }
    };
    let name = e
        .path
        .clone()
        .and_then(|p| p.rsplit(['/', '\\']).next().map(str::to_string))
        .unwrap_or_else(|| format!("{}.{}", cur.name, io::BUNDLE_EXTENSION));
    let store = Rc::clone(store);
    wasm_bindgen_futures::spawn_local(async move {
        let mut err = None;
        let mut written = Vec::new();
        // The audio first, then the session that points at it: a visit cut short never finds a
        // session missing its audio.
        for (path, bytes) in &parts.new_files {
            match js_put(path, bytes).await {
                Ok(_) => written.push(path.clone()),
                Err(e) => {
                    err = Some(js_text(&e));
                    break;
                }
            }
        }
        if err.is_none() {
            let meta = serde_json::json!({"name": name}).to_string();
            for (path, bytes) in [(SESSION, parts.json.as_bytes()), (META, meta.as_bytes())] {
                if let Err(e) = js_put(path, bytes).await {
                    err = Some(js_text(&e));
                    break;
                }
            }
        }
        let old: Vec<String> = {
            let s = store.borrow();
            s.stored.iter().filter(|p| p.starts_with("Audio Files/") && !parts.used.contains(p)).cloned().collect()
        };
        if err.is_none() {
            for p in &old {
                let _ = js_remove(p).await;
            }
        }
        let mut s = store.borrow_mut();
        s.stored.extend(written);
        if err.is_none() {
            s.stored.retain(|p| !old.contains(p));
            s.stored.insert(SESSION.into());
            s.stored.insert(META.into());
            s.failed = None;
        } else {
            // Kept again at the next change.
            s.kept = None;
        }
        if let Some(e) = err {
            s.failed = Some(e);
        }
        s.busy = false;
    });
}

/// Clear: the kept copy is forgotten (the open session stays open, and is kept again at its next
/// change).
pub fn clear(store: &Shared, e: &Engine) {
    {
        let mut s = store.borrow_mut();
        s.kept = Some(e.session_arc());
        s.busy = true;
    }
    let store = Rc::clone(store);
    wasm_bindgen_futures::spawn_local(async move {
        let r = js_clear().await;
        let mut s = store.borrow_mut();
        s.stored.clear();
        s.busy = false;
        s.said = Some(match r {
            Ok(_) => "Removed the copy kept in this browser".into(),
            Err(e) => format!("Could not clear this browser's copy: {}", js_text(&e)),
        });
    });
}
