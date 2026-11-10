//! The song engines' servers, run by FreeDAWm itself: an installed engine folder
//! (`<models>/<id>/`, downloaded from Setup › ML Models) holds the server program (yue2.cpp's
//! `yue-server`, acestep.cpp's `ace-server`, built for each platform by the engines workflow) and
//! its GGUF models; its `card.json` says how to start it:
//!
//! ```json
//! { "file": "bin/yue-server", "task": "song_server",
//!   "server_args": ["--model", "{dir}/models/YuE2-3B-Q8_0.gguf", "--vae", "{dir}/models/YuE2-Vae-F32.gguf",
//!                   "--host", "127.0.0.1", "--port", "{port}"],
//!   "device_arg": "--device", "cpu_device": "CPU" }
//! ```
//!
//! [`acquire`] starts the server on a free port of this computer (only 127.0.0.1) the first time
//! a song needs it and waits for `/health`; later songs reuse it. It stays up while songs use it
//! and stops after [`IDLE`] without one (freeing the GPU's memory), when it exits on its own, or
//! at [`shutdown_all`] (FreeDAWm quitting). A server that crashes takes only that song with it.

use crate::models::InstalledModel;
use crate::runtime::Device;
use crate::{Cancel, MlError, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a server stays up without a song.
pub const IDLE: Duration = Duration::from_secs(600);
/// How long a server may take to load before it answers `/health`.
pub const START_TIMEOUT: Duration = Duration::from_secs(180);

struct Server {
    id: String,
    dir: PathBuf,
    device: Device,
    port: u16,
    child: Child,
    log: PathBuf,
    busy: Arc<std::sync::atomic::AtomicUsize>,
    last_used: Instant,
}

fn servers() -> &'static Mutex<Vec<Server>> {
    static S: OnceLock<Mutex<Vec<Server>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

/// A server in use: its address. The server is not stopped for idleness while one is held.
#[derive(Debug)]
pub struct Lease {
    pub url: String,
    busy: Arc<std::sync::atomic::AtomicUsize>,
    id: String,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.busy.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut list) = servers().lock()
            && let Some(s) = list.iter_mut().find(|s| s.id == self.id)
        {
            s.last_used = Instant::now();
        }
    }
}

fn bad_card(id: &str, what: &str) -> MlError {
    MlError::Model(id.into(), format!("its card.json {what}: reinstall it from Setup › ML Models"))
}

/// The server's command line from its card: `{dir}` is the engine folder, `{port}` the port.
pub fn command_line(model: &InstalledModel, port: u16, device: Device) -> Result<(PathBuf, Vec<String>)> {
    let id = model.card.id.as_str();
    let exe = model.file(&model.card.file)?;
    let dir = model.dir.to_string_lossy().to_string();
    let args = model.card.value("server_args").and_then(serde_json::Value::as_array).ok_or_else(|| bad_card(id, "has no `server_args`"))?;
    let mut out: Vec<String> = Vec::with_capacity(args.len() + 2);
    for a in args {
        let a = a.as_str().ok_or_else(|| bad_card(id, "has a `server_args` entry that is not text"))?;
        out.push(a.replace("{dir}", &dir).replace("{port}", &port.to_string()));
    }
    // The device: the card's CPU name when the person chose the CPU, else the server's best.
    if device == Device::Cpu
        && let (Some(flag), Some(cpu)) = (model.card.text("device_arg"), model.card.text("cpu_device"))
    {
        out.push(flag.to_string());
        out.push(cpu.to_string());
    }
    Ok((exe, out))
}

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| MlError::Io(format!("no free port on this computer: {e}")))?;
    l.local_addr().map(|a| a.port()).map_err(|e| MlError::Io(format!("no free port on this computer: {e}")))
}

/// Unpacked programs lose their executable bit (zips don't keep it): give it back to the folder's
/// `bin/` files.
#[cfg(unix)]
fn make_executable(exe: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut files = vec![exe.to_path_buf()];
    if let Some(Ok(rd)) = exe.parent().map(std::fs::read_dir) {
        files.extend(rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()));
    }
    for f in files {
        if let Ok(meta) = std::fs::metadata(&f) {
            let mut p = meta.permissions();
            if p.mode() & 0o111 == 0 {
                p.set_mode(p.mode() | 0o755);
                let _ = std::fs::set_permissions(&f, p);
            }
        }
    }
}

#[cfg(not(unix))]
fn make_executable(_: &Path) {}

/// The last lines of a server's log (for an error message).
fn log_tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines.iter().skip(lines.len().saturating_sub(4)).copied().collect::<Vec<_>>().join(" | ")
}

fn healthy(url: &str) -> bool {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build().get(&format!("{url}/health")).call().is_ok_and(|r| r.status() == 200)
}

fn kill(s: &mut Server) {
    let _ = s.child.kill();
    let _ = s.child.wait();
}

/// The running server of engine `model` (started now if it isn't running, or was started from
/// another folder or for another device), leased while the song runs. Waits for it to answer,
/// cancellably; `say` gets what is happening.
pub fn acquire(model: &InstalledModel, device: Device, cancel: &Cancel, say: &dyn Fn(&str)) -> Result<Lease> {
    reaper();
    let id = model.card.id.clone();
    let mut list = servers().lock().map_err(|_| MlError::Runtime("the song servers' list is unusable".into()))?;
    // Forget servers that exited; keep a live one for this engine, folder and device.
    list.retain_mut(|s| match s.child.try_wait() {
        Ok(None) => true,
        _ => {
            log::warn!("song server `{}` had exited: {}", s.id, log_tail(&s.log));
            false
        }
    });
    if let Some(i) = list.iter().position(|s| s.id == id) {
        let same = list.get(i).is_some_and(|s| s.dir == model.dir && s.device == device);
        if same && let Some(s) = list.get_mut(i) {
            s.busy.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            s.last_used = Instant::now();
            return Ok(Lease { url: format!("http://127.0.0.1:{}", s.port), busy: Arc::clone(&s.busy), id });
        }
        let mut old = list.remove(i);
        kill(&mut old);
    }
    let port = free_port()?;
    let (exe, args) = command_line(model, port, device)?;
    make_executable(&exe);
    let log = std::env::temp_dir().join(format!("freedawm-{id}-server.log"));
    let out = std::fs::File::create(&log).map_err(|e| MlError::Io(format!("{}: {e}", log.display())))?;
    let err = out.try_clone().map_err(|e| MlError::Io(format!("{}: {e}", log.display())))?;
    let mut cmd = Command::new(&exe);
    cmd.args(&args).current_dir(&model.dir).stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    // Its libraries (ggml and the backends) sit beside it: let the loader look there too.
    if let Some(bin) = exe.parent() {
        let var = if cfg!(target_os = "macos") { "DYLD_LIBRARY_PATH" } else { "LD_LIBRARY_PATH" };
        if !cfg!(windows) {
            let mut paths = vec![bin.to_path_buf()];
            if let Some(old) = std::env::var_os(var) {
                paths.extend(std::env::split_paths(&old));
            }
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env(var, joined);
            }
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window.
        cmd.creation_flags(0x0800_0000);
    }
    say(&format!("Starting {}", model.card.name));
    let child = cmd.spawn().map_err(|e| MlError::Runtime(format!("can't start {}: {e}", exe.display())))?;
    let busy = Arc::new(std::sync::atomic::AtomicUsize::new(1));
    let mut server = Server { id: id.clone(), dir: model.dir.clone(), device, port, child, log, busy: Arc::clone(&busy), last_used: Instant::now() };
    let url = format!("http://127.0.0.1:{port}");
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            kill(&mut server);
            return Err(MlError::Cancelled);
        }
        if let Ok(Some(status)) = server.child.try_wait() {
            return Err(MlError::Runtime(format!("the {} engine stopped while starting ({status}): {}", model.card.name, log_tail(&server.log))));
        }
        if healthy(&url) {
            break;
        }
        if started.elapsed() > START_TIMEOUT {
            kill(&mut server);
            return Err(MlError::Runtime(format!(
                "the {} engine did not start in {} s: {}",
                model.card.name,
                START_TIMEOUT.as_secs(),
                log_tail(&server.log)
            )));
        }
        say(&format!("Starting {} ({} s)", model.card.name, started.elapsed().as_secs()));
        std::thread::sleep(Duration::from_millis(250));
    }
    list.push(server);
    Ok(Lease { url, busy, id })
}

/// Stops servers idle longer than [`IDLE`] (a thread started once).
fn reaper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let _ = std::thread::Builder::new().name("song-servers".into()).spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(20));
                stop_idle(IDLE);
            }
        });
    });
}

/// Stops the servers no song has used for `idle` (none in use).
pub fn stop_idle(idle: Duration) {
    if let Ok(mut list) = servers().lock() {
        list.retain_mut(|s| {
            let unused = s.busy.load(std::sync::atomic::Ordering::SeqCst) == 0 && s.last_used.elapsed() >= idle;
            if unused {
                kill(s);
            }
            !unused
        });
    }
}

/// Stops every server (FreeDAWm quitting).
pub fn shutdown_all() {
    if let Ok(mut list) = servers().lock() {
        for s in list.iter_mut() {
            kill(s);
        }
        list.clear();
    }
}

/// The engines running now: (id, address).
pub fn running() -> Vec<(String, String)> {
    servers().lock().map(|l| l.iter().map(|s| (s.id.clone(), format!("http://127.0.0.1:{}", s.port))).collect()).unwrap_or_default()
}
