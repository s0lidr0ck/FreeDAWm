//! Model downloads in the app (spec 2026-11-04-model-downloads): the manifest, the download
//! queue on a worker thread, the personal-use confirm, and the pieces the ML Models window and the
//! job dialogs show. Nothing starts without a click (or a control call): no background or
//! automatic downloads. The manifest is fetched when the ML Models window opens or a Download
//! button is pressed.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use serde_json::{Value, json};
use soundcraft_engine::ml::catalog::{Manifest, ManifestModel, is_newer, is_personal_use};
use soundcraft_engine::ml::download::{self, DownloadProgress};
use soundcraft_engine::ml::{self, Cancel, ModelStatus};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Instant;

/// Where the manifest stands.
#[derive(Default)]
pub enum ManifestState {
    #[default]
    NotLoaded,
    Loading(Receiver<Result<Manifest, String>>),
    Ready(Manifest),
    Failed(String),
}

/// The download on its worker.
pub struct Active {
    pub id: String,
    cancel: Cancel,
    pub progress: Arc<DownloadProgress>,
    rx: Receiver<Result<(), String>>,
    started: Instant,
    /// Renders: a download that never ends (keeps the channel open).
    _hold: Option<Sender<Result<(), String>>>,
}

/// The downloads' state (in [`crate::ml_ui::MlUi`]).
#[derive(Default)]
pub struct Downloads {
    pub manifest: ManifestState,
    pub active: Option<Active>,
    pub queue: VecDeque<String>,
    /// Personal-use models waiting for the confirm, with the others asked for at the same click.
    pub confirm: Option<Confirm>,
    /// Asked for before the manifest was there: queued once it is (confirmed or not).
    pending: Option<(Vec<String>, bool)>,
    /// The last outcome per model ("download failed: …"), shown on its row.
    pub notes: Vec<(String, String)>,
    /// Another model folder and manifest URL (tests and renders); None: the app's.
    pub dir: Option<PathBuf>,
    pub url: Option<String>,
}

/// The personal-use confirm.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    /// The personal-use models asked for.
    pub personal: Vec<String>,
    /// The others asked for at the same click (downloaded whatever the answer).
    pub others: Vec<String>,
}

/// The model folder downloads go to.
pub fn models_dir(app: &SoundApp) -> PathBuf {
    app.ml.downloads.dir.clone().unwrap_or_else(ml::models_dir)
}

fn manifest_url(app: &SoundApp) -> String {
    app.ml.downloads.url.clone().unwrap_or_else(download::manifest_url)
}

/// Uses another model folder and manifest URL (tests and renders).
pub fn set_source(app: &mut SoundApp, dir: PathBuf, url: Option<String>) {
    app.ml.downloads.dir = Some(dir);
    app.ml.downloads.url = url;
    app.ml.downloads.manifest = ManifestState::NotLoaded;
    app.ml.models = None;
}

/// Starts fetching the manifest unless it is there or on its way (`again`: fetch it anew).
pub fn load_manifest(app: &mut SoundApp, again: bool) {
    let d = &mut app.ml.downloads;
    match d.manifest {
        ManifestState::Loading(_) => return,
        ManifestState::Ready(_) if !again => return,
        _ => {}
    }
    let url = app.ml.downloads.url.clone().unwrap_or_else(download::manifest_url);
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("ml-manifest".into()).spawn(move || {
        let _ = tx.send(download::fetch_manifest(&url).map_err(|e| e.to_string()));
    });
    app.ml.downloads.manifest = match spawned {
        Ok(_) => ManifestState::Loading(rx),
        Err(e) => ManifestState::Failed(e.to_string()),
    };
}

/// The manifest, fetched on this thread if it isn't there (control calls only: never the UI).
fn manifest_now(app: &mut SoundApp, again: bool) -> Result<Manifest, String> {
    if let (ManifestState::Ready(m), false) = (&app.ml.downloads.manifest, again) {
        return Ok(m.clone());
    }
    let got = download::fetch_manifest(&manifest_url(app)).map_err(|e| e.to_string());
    app.ml.downloads.manifest = match &got {
        Ok(m) => ManifestState::Ready(m.clone()),
        Err(e) => ManifestState::Failed(e.clone()),
    };
    got
}

/// The manifest when it is there.
pub fn manifest(app: &SoundApp) -> Option<&Manifest> {
    match &app.ml.downloads.manifest {
        ManifestState::Ready(m) => Some(m),
        _ => None,
    }
}

fn personal(app: &SoundApp, id: &str) -> bool {
    manifest(app).and_then(|m| m.get(id)).map_or_else(|| is_personal_use(ml::known_licence(id)), |e| e.personal_use_only)
}

fn note(app: &mut SoundApp, id: &str, text: String) {
    let n = &mut app.ml.downloads.notes;
    n.retain(|(i, _)| i != id);
    if !text.is_empty() {
        n.push((id.to_string(), text));
    }
}

/// The last outcome noted for model `id`.
pub fn note_of<'a>(app: &'a SoundApp, id: &str) -> Option<&'a str> {
    app.ml.downloads.notes.iter().find(|(i, _)| i == id).map(|(_, t)| t.as_str())
}

/// A click on Download (or Download all, or a dialog's Download): queues `ids`, asking first
/// for the personal-use ones unless `confirmed`. Before the manifest is there it is fetched and
/// the request waits for it.
pub fn request(app: &mut SoundApp, ids: &[String], confirmed: bool) {
    if manifest(app).is_none() {
        app.ml.downloads.pending = Some((ids.to_vec(), confirmed));
        load_manifest(app, false);
        return;
    }
    let (personal_ids, others): (Vec<String>, Vec<String>) = ids.iter().cloned().partition(|id| personal(app, id));
    if !confirmed && !personal_ids.is_empty() {
        app.ml.downloads.confirm = Some(Confirm { personal: personal_ids, others });
        return;
    }
    enqueue(app, ids);
}

fn enqueue(app: &mut SoundApp, ids: &[String]) {
    for id in ids {
        let ok = manifest(app).and_then(|m| m.get(id)).is_some_and(ManifestModel::downloadable);
        let d = &app.ml.downloads;
        if !ok || d.queue.contains(id) || d.active.as_ref().is_some_and(|a| &a.id == id) {
            continue;
        }
        app.ml.downloads.queue.push_back(id.clone());
        note(app, id, String::new());
    }
    start_next(app);
}

fn start_next(app: &mut SoundApp) {
    if app.ml.downloads.active.is_some() {
        return;
    }
    while let Some(id) = app.ml.downloads.queue.pop_front() {
        let Some(entry) = manifest(app).and_then(|m| m.get(&id)).cloned() else { continue };
        let (dir, url) = (models_dir(app), manifest_url(app));
        let (cancel, progress) = (Cancel::new(), DownloadProgress::new());
        progress.set(0, entry.size);
        let (c, p) = (cancel.clone(), progress.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name(format!("ml-download-{id}")).spawn(move || {
            let r = std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()));
            let r = r.and_then(|()| download::download(&dir, &entry, &url, &c, &p).map_err(|e| e.to_string()));
            let _ = tx.send(r);
        });
        match spawned {
            Ok(_) => {
                app.ml.downloads.active = Some(Active { id, cancel, progress, rx, started: Instant::now(), _hold: None });
                return;
            }
            Err(e) => note(app, &id, format!("download failed: {e}")),
        }
    }
}

/// Cancels the download and empties the queue.
pub fn cancel_all(app: &mut SoundApp) {
    app.ml.downloads.queue.clear();
    if let Some(a) = &app.ml.downloads.active {
        a.cancel.cancel();
    }
}

/// Removes an installed model (not while it downloads or a job runs).
pub fn remove(app: &mut SoundApp, id: &str) -> Result<(), String> {
    if app.ml.running.is_some() {
        return Err("Finish or cancel the ML job first".into());
    }
    if app.ml.downloads.active.as_ref().is_some_and(|a| a.id == id) {
        return Err(format!("`{id}` is downloading: cancel it first"));
    }
    let r = download::remove(&models_dir(app), id).map_err(|e| e.to_string());
    app.ml.models = None;
    r
}

/// Picks up the manifest and finished downloads. Called every frame.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    if let ManifestState::Loading(rx) = &app.ml.downloads.manifest {
        match rx.try_recv() {
            Ok(Ok(m)) => app.ml.downloads.manifest = ManifestState::Ready(m),
            Ok(Err(e)) => app.ml.downloads.manifest = ManifestState::Failed(e),
            Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            Err(TryRecvError::Disconnected) => app.ml.downloads.manifest = ManifestState::Failed("the manifest fetch stopped".into()),
        }
        if !matches!(app.ml.downloads.manifest, ManifestState::Loading(_)) {
            match app.ml.downloads.pending.take() {
                Some((ids, confirmed)) if manifest(app).is_some() => request(app, &ids, confirmed),
                _ => {}
            }
        }
    }
    let Some(a) = &app.ml.downloads.active else { return };
    let got = match a.rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            return;
        }
        Err(TryRecvError::Disconnected) => Err("the download stopped unexpectedly".into()),
    };
    let Some(a) = app.ml.downloads.active.take() else { return };
    let name = ml::KNOWN_MODELS.iter().find(|m| m.id == a.id).map_or(a.id.as_str(), |m| m.name);
    match got {
        Ok(()) => {
            note(app, &a.id, String::new());
            app.ui.status = format!("Downloaded {name} ({:.0} s)", a.started.elapsed().as_secs_f64());
        }
        Err(_) if a.cancel.is_cancelled() => note(app, &a.id, "download cancelled".into()),
        Err(e) => {
            app.ui.status = format!("{name}: download failed: {e}");
            note(app, &a.id, format!("download failed: {e}"));
        }
    }
    app.ml.models = None;
    start_next(app);
}

/// "372 / 886 MB · 18 MB/s" for the download.
pub fn progress_text(a: &Active) -> String {
    let (done, total) = a.progress.get();
    if a.progress.installing() {
        return "checking and unpacking…".into();
    }
    let secs = a.started.elapsed().as_secs_f64().max(0.1);
    format!("{:.0} / {:.0} MB · {:.0} MB/s", done as f64 / 1e6, total as f64 / 1e6, done as f64 / 1e6 / secs)
}

fn fraction(a: &Active) -> f32 {
    let (done, total) = a.progress.get();
    if total == 0 { 0.0 } else { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 }
}

/// "687 MB", "1.2 GB".
pub fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000_000 { format!("{:.1} GB", bytes as f64 / 1e9) } else { format!("{:.0} MB", (bytes as f64 / 1e6).max(1.0)) }
}

/// The models a click on Download all fetches: every downloadable one not installed.
pub fn all_missing(app: &SoundApp, models: &[ml::ModelEntry]) -> Vec<String> {
    let Some(m) = manifest(app) else { return Vec::new() };
    models
        .iter()
        .filter(|e| matches!(e.status, ModelStatus::Missing | ModelStatus::Broken(_)))
        .filter(|e| m.get(e.model.id).is_some_and(ManifestModel::downloadable))
        .map(|e| e.model.id.to_string())
        .collect()
}

fn total_size(app: &SoundApp, ids: &[String]) -> u64 {
    ids.iter().filter_map(|id| manifest(app).and_then(|m| m.get(id))).map(|e| e.size).sum()
}

/// Whether model `id` (installed) has a newer version in the release.
pub fn update_available(app: &SoundApp, entry: &ml::ModelEntry) -> bool {
    match (&entry.status, manifest(app).and_then(|m| m.get(entry.model.id))) {
        (ModelStatus::Installed(card), Some(e)) => e.downloadable() && is_newer(&e.version, &download::card_version(card)),
        _ => false,
    }
}

/// The top line of the ML Models window: where the models come from, what's left to download and
/// Download all; or why the release can't be reached and the manual route.
pub fn header(app: &mut SoundApp, ui: &mut egui::Ui, models: &[ml::ModelEntry]) {
    let t = Tokens::current();
    match &app.ml.downloads.manifest {
        ManifestState::NotLoaded | ManifestState::Loading(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Looking up the FreeDAWm models release…").color(t.text_dim));
            });
        }
        ManifestState::Failed(e) => {
            let e = e.clone();
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(t.rec, format!("Can't reach the FreeDAWm models release ({e}). Install the models by hand instead (below)."));
                if ui.small_button("Try again").clicked() {
                    load_manifest(app, true);
                }
            });
        }
        ManifestState::Ready(_) => {
            let ids = all_missing(app, models);
            let total = total_size(app, &ids);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Setup › ML Models").font(bold(13.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = egui::RichText::new("Download all").color(t.mode_on_text);
                    if ui
                        .add_enabled(!ids.is_empty(), egui::Button::new(label).fill(t.mode_on))
                        .on_hover_text("Every model not installed; personal-use models ask first")
                        .clicked()
                    {
                        request(app, &ids, false);
                    }
                    let what = if ids.is_empty() { "everything installed".to_string() } else { format!("{} not installed", size_text(total)) };
                    ui.label(egui::RichText::new(format!("From the FreeDAWm models release · {what}")).color(t.text_dim));
                });
            });
        }
    }
}

/// The Status cell of a model row: the download's progress bar, or its last outcome. False when
/// the row should show its usual status.
pub fn status_cell(app: &SoundApp, ui: &mut egui::Ui, id: &str) -> bool {
    let t = Tokens::current();
    if let Some(a) = app.ml.downloads.active.as_ref().filter(|a| a.id == id) {
        ui.vertical(|ui| {
            ui.add(egui::ProgressBar::new(fraction(a)).desired_width(150.0).desired_height(6.0));
            ui.label(egui::RichText::new(progress_text(a)).small().color(t.text));
        });
        return true;
    }
    if app.ml.downloads.queue.iter().any(|q| q == id) {
        ui.label(egui::RichText::new("queued").color(t.text_dim));
        return true;
    }
    if let Some(n) = note_of(app, id) {
        ui.colored_label(t.rec, n).on_hover_text(n);
        return true;
    }
    false
}

/// What a row's button asked for.
pub enum RowAction {
    Download(String),
    Cancel,
    Remove(String),
}

/// The action cell of a model row: Download (size), Cancel, Update and Remove, or how to install
/// it by hand.
pub fn action_cell(app: &SoundApp, ui: &mut egui::Ui, entry: &ml::ModelEntry) -> Option<RowAction> {
    let id = entry.model.id;
    let t = Tokens::current();
    if matches!(entry.status, ModelStatus::Builtin) {
        return None;
    }
    if app.ml.downloads.active.as_ref().is_some_and(|a| a.id == id) || app.ml.downloads.queue.iter().any(|q| q == id) {
        return ui.button("Cancel").clicked().then_some(RowAction::Cancel);
    }
    let release = manifest(app).and_then(|m| m.get(id));
    let mut act = None;
    ui.horizontal(|ui| match &entry.status {
        ModelStatus::Installed(_) => {
            if update_available(app, entry) && ui.button("Update").on_hover_text("A newer version is in the models release").clicked() {
                act = Some(RowAction::Download(id.to_string()));
            }
            if ui.button("Remove").on_hover_text(format!("Deletes {}", entry.dir.display())).clicked() {
                act = Some(RowAction::Remove(id.to_string()));
            }
        }
        _ => match release {
            Some(e) if e.downloadable() => {
                if ui.button(format!("Download {}", size_text(e.size))).clicked() {
                    act = Some(RowAction::Download(id.to_string()));
                }
            }
            Some(e) => {
                ui.label(egui::RichText::new("Install by hand").color(t.text_dim)).on_hover_text(&e.manual_note);
            }
            None => {}
        },
    });
    act
}

/// The job dialog's offer for its missing models: Download (size), or the progress of theirs.
/// True when the dialog should keep showing "See Setup › ML Models" (no way to download here).
pub fn dialog_offer(app: &mut SoundApp, ui: &mut egui::Ui, missing: &[&str]) -> bool {
    let t = Tokens::current();
    if let Some(a) = app.ml.downloads.active.as_ref().filter(|a| missing.contains(&a.id.as_str())) {
        let (text, f) = (progress_text(a), fraction(a));
        let name = ml::KNOWN_MODELS.iter().find(|m| m.id == a.id).map_or(a.id.as_str(), |m| m.name);
        ui.label(format!("Downloading {name}"));
        ui.horizontal(|ui| {
            ui.add(egui::ProgressBar::new(f).desired_width(180.0).desired_height(6.0));
            ui.label(egui::RichText::new(text).small().color(t.text_dim));
            if ui.small_button("Cancel").clicked() {
                cancel_all(app);
            }
        });
        return false;
    }
    if missing.iter().any(|id| app.ml.downloads.queue.iter().any(|q| q == id)) {
        ui.label(egui::RichText::new("Queued for download…").color(t.text_dim));
        return false;
    }
    let ids: Vec<String> = missing.iter().map(|s| s.to_string()).collect();
    let (label, can) = match &app.ml.downloads.manifest {
        ManifestState::Ready(m) => {
            let all = ids.iter().all(|id| m.get(id).is_some_and(ManifestModel::downloadable));
            (format!("Download ({})", size_text(total_size(app, &ids))), all)
        }
        ManifestState::Failed(_) => ("Download".to_string(), true),
        _ => ("Download".to_string(), true),
    };
    if let Some(n) = missing.iter().find_map(|id| note_of(app, id)) {
        ui.colored_label(t.rec, n.to_string());
    }
    if can {
        if ui.button(label).on_hover_text("Downloads the missing models from the FreeDAWm models release; then Run").clicked() {
            request(app, &ids, false);
        }
        if let ManifestState::Failed(e) = &app.ml.downloads.manifest {
            ui.colored_label(t.rec, format!("Can't reach the models release: {e}"));
            return true;
        }
        return false;
    }
    true
}

/// The personal-use confirm, when one is asked.
pub fn confirm_window(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(c) = app.ml.downloads.confirm.clone() else { return };
    let t = Tokens::current();
    let mut answer: Option<Option<bool>> = None;
    egui::Modal::new(egui::Id::new("ml_download_confirm")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.label(egui::RichText::new("Personal use only").font(bold(13.0)));
        for id in &c.personal {
            let e = manifest(app).and_then(|m| m.get(id)).cloned().unwrap_or_default();
            let name = ml::KNOWN_MODELS.iter().find(|m| m.id == id.as_str()).map_or(id.as_str(), |m| m.name);
            let status = if e.license.is_empty() { ml::known_licence(id).to_string() } else { e.license.clone() };
            let why =
                if status.contains("non-commercial") { "The author allows non-commercial use only." } else { "The author hasn't stated a licence." };
            ui.label(egui::RichText::new(format!("{name} ({})", size_text(e.size))).strong());
            ui.colored_label(t.rec, format!("{why} Use it for personal projects only."));
            if !e.source.is_empty() {
                ui.label(egui::RichText::new(format!("Source: {}", e.source)).small().color(t.text_dim));
            }
        }
        ui.label(if c.personal.len() == 1 { "Download it?" } else { "Download them?" });
        ui.horizontal(|ui| {
            if ui.button("Download").clicked() {
                answer = Some(Some(true));
            }
            if !c.others.is_empty() && ui.button(format!("Skip {}", if c.personal.len() == 1 { "it" } else { "these" })).clicked() {
                answer = Some(Some(false));
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = Some(None);
            }
        });
    });
    match answer {
        Some(Some(true)) => {
            app.ml.downloads.confirm = None;
            let ids: Vec<String> = c.others.iter().chain(&c.personal).cloned().collect();
            enqueue(app, &ids);
        }
        Some(Some(false)) => {
            app.ml.downloads.confirm = None;
            enqueue(app, &c.others);
        }
        Some(None) => app.ml.downloads.confirm = None,
        None => {}
    }
}

/// Whether `id` is one of the download control methods (never a job: no dialog, not blocked
/// while a job runs).
pub fn is_download_command(id: &str) -> bool {
    matches!(id, "ml.models_available" | "ml.download" | "ml.download_cancel" | "ml.remove")
}

/// The control methods: `ml.models_available`, `ml.download`, `ml.download_cancel`, `ml.remove`.
pub fn command(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ml.models_available" => {
            let refresh = p.get("refresh").and_then(Value::as_bool).unwrap_or(false);
            let fetched = manifest_now(app, refresh);
            Ok(available(app, fetched.err()))
        }
        "ml.download" => {
            let ids: Vec<String> = match (p.get("ids").and_then(Value::as_array), p.get("id").and_then(Value::as_str)) {
                (Some(a), _) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                (None, Some(one)) => vec![one.to_string()],
                (None, None) => return Some(Err("ml.download needs `ids` (or `id`)".into())),
            };
            let confirmed = p.get("confirm_personal_use").and_then(Value::as_bool).unwrap_or(false);
            match manifest_now(app, false) {
                Err(e) => Err(format!("can't reach the models release: {e}")),
                Ok(m) => {
                    if let Some(bad) = ids.iter().find(|id| !m.get(id).is_some_and(ManifestModel::downloadable)) {
                        return Some(Err(format!(
                            "`{bad}` is not in the models release{}",
                            m.get(bad).map_or(String::new(), |e| format!(": {}", e.manual_note))
                        )));
                    }
                    let personal: Vec<&String> = ids.iter().filter(|id| m.get(id).is_some_and(|e| e.personal_use_only)).collect();
                    if !confirmed && !personal.is_empty() {
                        Err(format!(
                            "personal use only (no licence stated, or non-commercial): {}; pass confirm_personal_use: true to download",
                            personal.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                        ))
                    } else {
                        enqueue(app, &ids);
                        Ok(json!({"queued": ids, "downloading": app.ml.downloads.active.as_ref().map(|a| a.id.clone())}))
                    }
                }
            }
        }
        "ml.download_cancel" => {
            let was = app.ml.downloads.active.as_ref().map(|a| a.id.clone());
            cancel_all(app);
            Ok(json!({"cancelled": was}))
        }
        "ml.remove" => match p.get("id").and_then(Value::as_str) {
            Some(m) => remove(app, m).map(|()| json!({"removed": m})),
            None => Err("ml.remove needs `id`".into()),
        },
        _ => return None,
    })
}

/// `ml.models_available`'s answer.
fn available(app: &mut SoundApp, error: Option<String>) -> Value {
    let dir = models_dir(app);
    let models = ml::list_models(&dir);
    let rows: Vec<Value> = models
        .iter()
        .map(|e| {
            let r = manifest(app).and_then(|m| m.get(e.model.id));
            let installed = match &e.status {
                ModelStatus::Installed(c) => download::card_version(c),
                _ => String::new(),
            };
            let status = match &e.status {
                ModelStatus::Installed(_) => "installed",
                ModelStatus::Builtin => "built in",
                ModelStatus::Missing => "not installed",
                ModelStatus::Broken(_) => "broken",
            };
            let active = app.ml.downloads.active.as_ref().filter(|a| a.id == e.model.id);
            json!({"id": e.model.id, "name": e.model.name, "status": status, "installed_version": installed,
                   "available": r.is_some_and(ManifestModel::downloadable), "version": r.map(|r| r.version.clone()), "size": r.map(|r| r.size),
                   "license": r.map_or_else(|| e.licence(), |r| r.license.clone()), "personal_use_only": personal(app, e.model.id),
                   "source": r.map(|r| r.source.clone()), "manual": r.is_some_and(|r| r.manual), "manual_note": r.map(|r| r.manual_note.clone()),
                   "update": update_available(app, e), "downloading": active.is_some(),
                   "progress": active.map(|a| { let (d, t) = a.progress.get(); json!({"done": d, "total": t, "installing": a.progress.installing()}) }),
                   "queued": app.ml.downloads.queue.iter().any(|q| q == e.model.id), "note": note_of(app, e.model.id)})
        })
        .collect();
    let state = match &app.ml.downloads.manifest {
        ManifestState::Ready(_) => "ready",
        ManifestState::Failed(_) => "failed",
        ManifestState::Loading(_) => "loading",
        ManifestState::NotLoaded => "not_loaded",
    };
    json!({"manifest_url": manifest_url(app), "manifest": state, "error": error, "folder": dir,
           "release": manifest(app).map(|m| m.release.clone()), "models": rows,
           "downloading": app.ml.downloads.active.as_ref().map(|a| a.id.clone()), "queue": app.ml.downloads.queue.iter().collect::<Vec<_>>()})
}

/// Renders: the ML Models window over a fake model folder and manifest (the catalogue's models at
/// their release sizes; HTDemucs and Basic Pitch installed, Basic Pitch with an update), with
/// Mel-Band RoFormer vocals downloading (372 of 886 MB). `confirm`: the personal-use confirm for
/// BS-RoFormer SW is open too. Nothing touches the network.
pub fn demo(app: &mut SoundApp, confirm: bool) -> Result<(), String> {
    const SIZES: [(&str, u64); 12] = [
        ("htdemucs", 174_563_130),
        ("bs_roformer_sw", 707_404_388),
        ("vocals_melroformer", 886_000_000),
        ("karaoke_melroformer", 917_231_469),
        ("drumsep_mdx23c", 437_795_078),
        ("srs_small", 42_899_008),
        ("basicpitch", 3_232_929),
        ("piano_kong", 154_209_785),
        ("adtof_drums", 3_124_250),
        ("game_vocal", 51_097_129),
        ("btc_chords", 50_796_388),
        ("whisper_lyrics", 0),
    ];
    let dir = std::env::temp_dir().join(format!("freedawm-models-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (id, version) in [("htdemucs", "2026-10-22T05:00:18Z"), ("basicpitch", "2026-10-01T00:00:00Z")] {
        let m = dir.join(id);
        std::fs::create_dir_all(&m).map_err(|e| e.to_string())?;
        std::fs::write(m.join("model.onnx"), b"demo").map_err(|e| e.to_string())?;
        let licence = if id == "basicpitch" { "Apache-2.0" } else { "MIT" };
        let card = json!({"id": id, "file": "model.onnx", "license": licence, "source": "https://example.invalid", "exported_utc": version});
        std::fs::write(m.join("card.json"), card.to_string()).map_err(|e| e.to_string())?;
    }
    set_source(app, dir.clone(), Some("http://127.0.0.1:9/models.json".into()));
    let models = SIZES
        .iter()
        .map(|(id, size)| {
            let known = ml::KNOWN_MODELS.iter().find(|m| m.id == *id);
            let licence = known.map_or("", |m| m.license);
            ManifestModel {
                id: id.to_string(),
                name: known.map_or("", |m| m.name).to_string(),
                version: if *id == "basicpitch" { "2026-10-22T11:17:46Z".into() } else { "2026-10-22T05:00:18Z".into() },
                url: format!("{id}.zip"),
                size: *size,
                sha256: "0".repeat(64),
                license: licence.to_string(),
                source: "https://example.invalid/author".into(),
                personal_use_only: is_personal_use(licence),
                manual: *size == 0,
                manual_note: if *size == 0 {
                    "Too large for the models release (4.7 GB): install it with `cargo xtask ml-models`.".into()
                } else {
                    String::new()
                },
                ..Default::default()
            }
        })
        .collect();
    app.ml.downloads.manifest = ManifestState::Ready(Manifest { format: 1, release: "models-v1".into(), models });
    let mut list = ml::list_models(&dir);
    for e in &mut list {
        if let Some((_, s)) = SIZES.iter().find(|(id, _)| *id == e.model.id).filter(|_| matches!(e.status, ModelStatus::Installed(_))) {
            e.size = *s;
        }
    }
    app.ml.models = Some(list);
    let (tx, rx) = std::sync::mpsc::channel();
    let progress = DownloadProgress::new();
    progress.set(372_000_000, 886_000_000);
    let started = Instant::now().checked_sub(std::time::Duration::from_secs_f64(372.0 / 18.0)).unwrap_or_else(Instant::now);
    app.ml.downloads.active = Some(Active { id: "vocals_melroformer".into(), cancel: Cancel::new(), progress, rx, started, _hold: Some(tx) });
    app.ml.downloads.confirm = confirm.then(|| Confirm { personal: vec!["bs_roformer_sw".into()], others: Vec::new() });
    app.ml.show_models = true;
    Ok(())
}
