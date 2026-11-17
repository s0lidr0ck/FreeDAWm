//! The app's side: [`BridgedPlugin`], a [`Plugin`] whose plug-in runs in the helper.

use crate::exchange::{self, Event, Host, MAX_CHANNELS, MAX_FRAMES, words_for};
use crate::proto::{Info, Prepare, Reply, Request, hex, unhex};
use crate::shm::Shm;
use crate::{HANG_SECONDS, HELPER_NAME};
use soundcraft_dsp::{Category, ParamInfo, Plugin, PluginInfo, Taper};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long a request on the pipe may take before the helper counts as hung.
const ANSWER_WITHIN: Duration = Duration::from_secs(HANG_SECONDS);

/// The helper next to the running program (`FREEDAWM_PLUGIN_HOST` names another), or why
/// there is none.
pub fn helper_exe() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("FREEDAWM_PLUGIN_HOST").filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return if p.is_file() { Ok(p) } else { Err(format!("{} (FREEDAWM_PLUGIN_HOST) does not exist", p.display())) };
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let name = format!("{HELPER_NAME}{}", std::env::consts::EXE_SUFFIX);
    let helper = exe.parent().map(|d| d.join(&name)).unwrap_or_else(|| PathBuf::from(&name));
    if helper.is_file() { Ok(helper) } else { Err(format!("{name} is missing: it must sit next to the app")) }
}

/// What the audio thread, the pipe's reader and the watchdog share.
struct Status {
    /// The helper is gone (crashed, hung and stopped, or closed): the plug-in is silenced.
    dead: AtomicBool,
    /// The audio thread found it behind for too long.
    hung: AtomicBool,
    /// The plug-in is being dropped (its helper's end is not a crash).
    closing: AtomicBool,
    name: Mutex<String>,
}

/// The pipe and the process.
struct Control {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    replies: Mutex<Receiver<String>>,
    status: Arc<Status>,
}

impl Control {
    /// One request and its reply; no reply within [`ANSWER_WITHIN`] stops the helper.
    fn call(&self, req: &Request) -> Result<Reply, String> {
        if self.status.dead.load(Ordering::Acquire) {
            return Err("the plug-in's process is gone".into());
        }
        let replies = self.replies.lock().unwrap_or_else(PoisonError::into_inner);
        // A reply left over from a request given up on is not this one's.
        while replies.try_recv().is_ok() {}
        let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        line.push('\n');
        let sent = self.stdin.lock().unwrap_or_else(PoisonError::into_inner).write_all(line.as_bytes());
        sent.map_err(|e| format!("the plug-in's process is gone: {e}"))?;
        let reply = match replies.recv_timeout(ANSWER_WITHIN) {
            Ok(r) => r,
            Err(_) => {
                self.status.hung.store(true, Ordering::Release);
                self.kill();
                return Err("the plug-in's process stopped answering".into());
            }
        };
        let reply: Reply = serde_json::from_str(&reply).map_err(|e| format!("the plug-in's process answered nonsense: {e}"))?;
        if reply.ok { Ok(reply) } else { Err(reply.error.unwrap_or_else(|| "refused".into())) }
    }

    fn kill(&self) {
        let mut c = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// A hosted plug-in in its own process (see the crate docs).
pub struct BridgedPlugin {
    info: &'static PluginInfo,
    control: Arc<Control>,
    status: Arc<Status>,
    /// The exchange's shared file and the audio thread's side of it, once prepared.
    shm: Option<Shm>,
    host: Option<Host>,
    /// Parameter values as last set, by the index of `info.params`.
    values: Vec<f32>,
    offline: bool,
    /// The offline flag the current exchange was prepared with.
    prepared_offline: bool,
    rate: f32,
    latency: usize,
    tail: usize,
}

static SEQ: AtomicU32 = AtomicU32::new(0);

fn shm_path() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{HELPER_NAME}-{}-{n}.shm", std::process::id()))
}

impl BridgedPlugin {
    /// Starts the helper and opens `id` in it (a `clap:` plug-in the scan cache knows, its
    /// bundle's folder handed over), prepared for 48 kHz stereo with 1024-frame blocks until
    /// [`Plugin::prepare`] says otherwise.
    pub fn spawn(id: &str) -> Result<BridgedPlugin, String> {
        let dir =
            soundcraft_clap_host::cached().and_then(|all| all.into_iter().find(|d| d.id == id)).and_then(|d| d.path.parent().map(Path::to_path_buf));
        BridgedPlugin::spawn_with(&helper_exe()?, id, dir.as_deref())
    }

    /// [`BridgedPlugin::spawn`] with the helper `exe` and the folder `dir` of the plug-in's
    /// bundle (tests; `id` may then be a built-in plug-in's).
    pub fn spawn_with(exe: &Path, id: &str, dir: Option<&Path>) -> Result<BridgedPlugin, String> {
        let mut child = Command::new(exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the plug-in's process has no pipe".into());
        };
        let status = Arc::new(Status {
            dead: AtomicBool::new(false),
            hung: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            name: Mutex::new(id.into()),
        });
        let (tx, rx) = channel();
        let st = Arc::clone(&status);
        // The pipe's reader: its end is the helper's end.
        std::thread::Builder::new()
            .name("plug-in pipe".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                st.dead.store(true, Ordering::Release);
                if !st.closing.load(Ordering::Acquire) {
                    let name = st.name.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    let how = if st.hung.load(Ordering::Acquire) { "stopped answering" } else { "crashed" };
                    crate::report(format!("{name} {how} in its own process: it is silenced, and the session goes on"));
                }
            })
            .map_err(|e| e.to_string())?;
        let control = Arc::new(Control { child: Mutex::new(child), stdin: Mutex::new(stdin), replies: Mutex::new(rx), status: Arc::clone(&status) });
        // The watchdog: a helper the audio thread found hung is stopped (the reader then says so).
        let (ctl, st) = (Arc::clone(&control), Arc::clone(&status));
        std::thread::Builder::new()
            .name("plug-in watchdog".into())
            .spawn(move || {
                while !st.closing.load(Ordering::Acquire) && !st.dead.load(Ordering::Acquire) {
                    if st.hung.load(Ordering::Acquire) {
                        ctl.kill();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .map_err(|e| e.to_string())?;
        let mut p = BridgedPlugin {
            info: placeholder_info(),
            control,
            status,
            shm: None,
            host: None,
            values: Vec::new(),
            offline: false,
            prepared_offline: false,
            rate: 48_000.0,
            latency: 0,
            tail: 0,
        };
        let (shm, prepare) = p.new_exchange(48_000.0, 1024, 2)?;
        let reply = p.control.call(&Request::Open { id: id.into(), dir: dir.map(|d| d.to_string_lossy().into_owned()), prepare })?;
        let info = reply.info.clone().ok_or("the plug-in's process gave no description")?;
        *p.status.name.lock().unwrap_or_else(PoisonError::into_inner) = info.name.clone();
        p.info = leaked_info(id, &info);
        p.values = p.info.params.iter().map(|q| q.default).collect();
        p.adopt(shm, &reply, 2, 1024);
        Ok(p)
    }

    /// Whether its process is gone and it is silenced.
    pub fn crashed(&self) -> bool {
        self.status.dead.load(Ordering::Acquire)
    }

    /// Blocks its process did not finish in time so far.
    pub fn overruns(&self) -> u64 {
        self.host.as_ref().map_or(0, Host::overruns)
    }

    /// Stops its process at once, as a crash would (tests).
    #[doc(hidden)]
    pub fn kill_helper(&self) {
        self.control.kill();
    }

    fn new_exchange(&self, rate: f32, max: usize, channels: usize) -> Result<(Shm, Prepare), String> {
        let (channels, max) = (channels.clamp(1, MAX_CHANNELS), max.clamp(1, MAX_FRAMES));
        let path = shm_path();
        let shm = Shm::create(&path, words_for(channels, max)).map_err(|e| format!("{}: {e}", path.display()))?;
        if !exchange::init(shm.words(), channels, max) {
            return Err("the shared block could not be set up".into());
        }
        Ok((shm, Prepare { rate, max, channels, offline: self.offline, shm: path.to_string_lossy().into_owned() }))
    }

    fn adopt(&mut self, shm: Shm, reply: &Reply, channels: usize, max: usize) {
        let mut host = Host::new(channels, max, self.info.is_instrument);
        // Parameters set before this exchange go with its first block.
        for (i, v) in self.values.iter().enumerate() {
            host.queue(Event::Param { index: u32::try_from(i).unwrap_or(u32::MAX), value: *v });
        }
        (self.shm, self.host) = (Some(shm), Some(host));
        (self.latency, self.tail, self.prepared_offline) = (reply.latency, reply.tail, self.offline);
    }

    fn silence(&self, io: &mut [Vec<f32>], frames: usize) {
        if self.info.is_instrument {
            io.iter_mut().for_each(|c| c.iter_mut().take(frames).for_each(|x| *x = 0.0));
        }
    }
}

impl Drop for BridgedPlugin {
    fn drop(&mut self) {
        self.status.closing.store(true, Ordering::Release);
        let _ = self.control.stdin.lock().unwrap_or_else(PoisonError::into_inner).write_all(b"{\"op\":\"quit\"}\n");
        self.control.kill();
    }
}

/// A description to stand in until the helper gives the real one.
fn placeholder_info() -> &'static PluginInfo {
    static P: PluginInfo = PluginInfo {
        id: "bridged",
        name: "Bridged plug-in",
        short_name: "Bridged",
        category: Category::Other,
        params: &[],
        is_instrument: false,
        audiosuite: false,
        sidechain: false,
        link: None,
        references: 0,
        midi_out: false,
    };
    &P
}

/// The plug-in's description as the helper gave it, leaked once per plug-in type and kept
/// (`PluginInfo` is `'static`, as `soundcraft-clap-host`'s are).
fn leaked_info(id: &str, i: &Info) -> &'static PluginInfo {
    static KNOWN: Mutex<Option<HashMap<String, &'static PluginInfo>>> = Mutex::new(None);
    let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    let known = known.get_or_insert_with(HashMap::new);
    if let Some(info) = known.get(id) {
        return info;
    }
    let s = |x: &str| -> &'static str { Box::leak(x.to_string().into_boxed_str()) };
    let params: Vec<ParamInfo> = i
        .params
        .iter()
        .map(|p| ParamInfo {
            id: s(&p.id),
            name: s(&p.name),
            min: p.min,
            max: p.max,
            default: p.default,
            unit: crate::helper::unit_named(&p.unit),
            taper: if p.log { Taper::Log } else { Taper::Linear },
            choices: Box::leak(p.choices.iter().map(|c| s(c)).collect::<Vec<_>>().into_boxed_slice()),
            display: None,
            group: s(&p.group),
        })
        .collect();
    let info: &'static PluginInfo = Box::leak(Box::new(PluginInfo {
        id: s(id),
        name: s(&i.name),
        short_name: s(&i.short_name),
        category: if i.instrument { Category::Instrument } else { Category::Other },
        params: Box::leak(params.into_boxed_slice()),
        is_instrument: i.instrument,
        audiosuite: false,
        sidechain: false,
        link: None,
        references: 0,
        midi_out: false,
    }));
    known.insert(id.into(), info);
    info
}

impl Plugin for BridgedPlugin {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }

    /// A new exchange of this shape, and the plug-in prepared for it, in its process (off the
    /// audio thread). If that fails the plug-in is silenced, as after a crash.
    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.rate = sample_rate;
        let (channels, max) = (channels.clamp(1, MAX_CHANNELS), max_block.clamp(1, MAX_FRAMES));
        let made = self.new_exchange(sample_rate, max, channels).and_then(|(shm, prepare)| {
            let reply = self.control.call(&Request::Prepare { prepare })?;
            Ok((shm, reply))
        });
        match made {
            Ok((shm, reply)) => self.adopt(shm, &reply, channels, max),
            Err(e) => {
                log::warn!("{}: {e}", self.info.name);
                (self.shm, self.host) = (None, None);
            }
        }
    }

    fn reset(&mut self) {
        if let Some(h) = &mut self.host {
            h.queue(Event::Reset);
        }
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = self.info.params.iter().position(|p| p.id == id) else { return false };
        let v = self.info.params.get(i).map_or(value, |p| p.clamp(value));
        if let Some(x) = self.values.get_mut(i) {
            *x = v;
        }
        if let Some(h) = &mut self.host {
            h.queue(Event::Param { index: u32::try_from(i).unwrap_or(u32::MAX), value: v });
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        let i = self.info.params.iter().position(|p| p.id == id)?;
        self.values.get(i).copied()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let (Some(shm), Some(host)) = (&self.shm, &mut self.host) else { return self.silence(io, frames) };
        if self.status.dead.load(Ordering::Acquire) {
            // Gone: an instrument is silent; an effect is its input, still one block late in
            // realtime (the ring stands each block in for itself, never waiting), so the latency
            // delay compensation lines up does not move.
            if self.info.is_instrument {
                return self.silence(io, frames);
            }
            if !self.prepared_offline {
                host.process(shm.words(), io, frames);
            }
            return;
        }
        if self.prepared_offline {
            let (since, status) = (Instant::now(), &self.status);
            host.process_waiting(shm.words(), io, frames, || {
                std::thread::yield_now();
                !status.dead.load(Ordering::Acquire) && since.elapsed() < ANSWER_WITHIN
            });
        } else {
            host.process(shm.words(), io, frames);
        }
        let hang = (HANG_SECONDS as f32 * self.rate.max(1.0)) as u64;
        if host.late_frames() > hang {
            self.status.hung.store(true, Ordering::Release);
            self.status.dead.store(true, Ordering::Release);
        }
    }

    fn latency(&self) -> usize {
        let bridge = if self.prepared_offline { 0 } else { self.host.as_ref().map_or(0, Host::latency) };
        self.latency.saturating_add(bridge)
    }

    fn tail_samples(&self) -> usize {
        self.tail
    }

    fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.midi_raw(offset, [0x90, note, velocity]);
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.midi_raw(offset, [0x80, note, 0]);
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.midi_raw(offset, msg);
    }

    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        if let Some(h) = &mut self.host {
            h.queue(Event::Midi { offset: u32::try_from(offset).unwrap_or(u32::MAX), msg });
        }
    }

    fn all_notes_off(&mut self) {
        if let Some(h) = &mut self.host {
            h.queue(Event::AllNotesOff);
        }
    }

    fn save_state(&mut self) -> Option<Vec<u8>> {
        unhex(&self.control.call(&Request::SaveState).ok()?.state?)
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        self.control.call(&Request::LoadState { state: hex(data) }).is_ok()
    }

    fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
        let control = Arc::clone(&self.control);
        Some(Box::new(move || unhex(&control.call(&Request::SaveState).ok()?.state?)))
    }
}
