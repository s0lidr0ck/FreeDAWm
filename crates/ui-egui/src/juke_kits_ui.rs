//! Recorded drum kits for Juke in the app: the kits installed on this computer, and the kits
//! release's list and downloads (on a worker thread, only after a click). Loading one is
//! `mix.kit_load {path}`.

use soundcraft_engine::ml::Cancel;
use soundcraft_engine::ml::download::DownloadProgress;
use soundcraft_engine::ml::kits::{self, KitEntry, KitsManifest};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

/// The kits release's list.
#[derive(Default)]
pub enum ListState {
    #[default]
    NotLoaded,
    Loading(Receiver<Result<KitsManifest, String>>),
    Ready(KitsManifest),
    Failed(String),
}

/// A kit downloading.
pub struct Active {
    pub id: String,
    pub progress: Arc<DownloadProgress>,
    cancel: Cancel,
    rx: Receiver<Result<(), String>>,
}

/// The recorded kits' state.
#[derive(Default)]
pub struct KitDownloads {
    pub list: ListState,
    pub active: Option<Active>,
    /// The last outcome ("SM Drums installed", "download failed: …").
    pub note: String,
    /// Another kits folder and list URL (tests); None: the app's.
    pub dir: Option<PathBuf>,
    pub url: Option<String>,
}

impl KitDownloads {
    pub fn dir(&self) -> PathBuf {
        self.dir.clone().unwrap_or_else(kits::kits_dir)
    }

    fn url(&self) -> String {
        self.url.clone().unwrap_or_else(kits::kits_manifest_url)
    }

    /// The kits installed: (id, name from its kit.json, folder).
    pub fn installed(&self) -> Vec<(String, String, PathBuf)> {
        kits::installed_kits(&self.dir())
            .into_iter()
            .map(|(id, path)| {
                let name = soundcraft_engine::juke_kits::read_recorded(&path).map_or_else(|_| id.clone(), |k| k.name);
                (id, name, path)
            })
            .collect()
    }

    /// Fetches the release's list (again), on a worker.
    pub fn load_list(&mut self) {
        if matches!(self.list, ListState::Loading(_)) {
            return;
        }
        let url = self.url();
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("kits-list".into()).spawn(move || {
            let _ = tx.send(kits::fetch_kits_manifest(&url).map_err(|e| e.to_string()));
        });
        self.list = match spawned {
            Ok(_) => ListState::Loading(rx),
            Err(e) => ListState::Failed(e.to_string()),
        };
    }

    /// Starts downloading kit `k` (one at a time).
    pub fn download(&mut self, k: &KitEntry) {
        if self.active.is_some() {
            return;
        }
        let (dir, url, entry) = (self.dir(), self.url(), k.clone());
        let cancel = Cancel::default();
        let progress = DownloadProgress::new();
        let (c2, p2) = (cancel.clone(), Arc::clone(&progress));
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("kit-download".into()).spawn(move || {
            let _ = tx.send(kits::download_kit(&dir, &entry, &url, &c2, &p2).map_err(|e| e.to_string()));
        });
        match spawned {
            Ok(_) => self.active = Some(Active { id: k.id.clone(), progress, cancel, rx }),
            Err(e) => self.note = format!("download failed: {e}"),
        }
    }

    pub fn cancel(&mut self) {
        if let Some(a) = &self.active {
            a.cancel.cancel();
        }
    }

    /// Takes in what the workers finished; true while one is still working (keep repainting).
    pub fn step(&mut self) -> bool {
        if let ListState::Loading(rx) = &self.list {
            match rx.try_recv() {
                Ok(Ok(m)) => self.list = ListState::Ready(m),
                Ok(Err(e)) => self.list = ListState::Failed(e),
                Err(TryRecvError::Disconnected) => self.list = ListState::Failed("the list could not be read".into()),
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(a) = &self.active {
            match a.rx.try_recv() {
                Ok(r) => {
                    self.note = match r {
                        Ok(()) => format!("{} installed", a.id),
                        Err(e) => format!("download failed: {e}"),
                    };
                    self.active = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.note = "download failed".into();
                    self.active = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        matches!(self.list, ListState::Loading(_)) || self.active.is_some()
    }
}
