//! The computer's physical memory, so the disk cache (`soundcraft_model::disk`) can size itself
//! to the machine: generous on a big PC, never pushing a small one into paging; the free space on
//! a disk, so recording can show how long it can go on; and the time zone's offset, so FreeDAWm
//! Recorder names its takes in the computer's local time.
//! It also keeps the computer awake while FreeDAWm is serving remotes ([`KeepAwake`]).
//!
//! This is an isolated `unsafe` helper crate (craftrules never-crash): `unsafe` is denied
//! crate-wide and allowed only in the private `ffi` module (a few OS calls per platform, each with
//! a `// SAFETY:` comment). The public API is safe and never panics.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[allow(unsafe_code)]
mod ffi;

/// Installed physical memory in bytes, when the platform says (Windows, macOS, Linux).
pub fn total_memory() -> Option<u64> {
    ffi::total_memory().filter(|b| *b > 0)
}

/// Bytes free for this user on the disk holding `path` (an existing file or folder), when the
/// platform says (Unix systems and Windows).
pub fn free_disk_space(path: &std::path::Path) -> Option<u64> {
    ffi::free_disk_space(path)
}

/// How far the computer's local time is ahead of UTC (seconds; negative west of Greenwich) at
/// `unix_secs` (seconds since 1970, UTC), daylight saving included, when the platform says.
///
/// Unix systems (Linux, macOS, FreeBSD) answer for that instant (`localtime_r`). Windows answers
/// with the offset in force now (its local clock less its UTC clock) whatever `unix_secs` is, so
/// a time on the other side of a daylight-saving change is an hour off there.
pub fn utc_offset_secs(unix_secs: i64) -> Option<i32> {
    ffi::utc_offset_secs(unix_secs).filter(|o| o.unsigned_abs() <= 18 * 3600)
}

/// Keeps the computer awake while held: it does not go to sleep on its own (the screen may still
/// turn off), and may again once released or dropped. Make and change it from one long-lived
/// thread (the UI thread): on Windows the request belongs to the thread that made it.
///
/// - Windows: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`.
/// - macOS: `caffeinate -i -w <this process>` (part of macOS), stopped on release; it ends by
///   itself if FreeDAWm does.
/// - Linux: `systemd-inhibit --what=idle:sleep` around `tail --pid=<this process>` where systemd
///   is there (most desktops); else nothing can be held and [`KeepAwake::set`] says so.
/// - Elsewhere: nothing.
#[derive(Debug, Default)]
pub struct KeepAwake {
    held: bool,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    child: Option<std::process::Child>,
}

impl KeepAwake {
    /// Holds (`true`) or releases. `Ok(true)` while held; `Err` with why it could not be.
    pub fn set(&mut self, on: bool, why: &str) -> Result<bool, String> {
        if on == self.held {
            return Ok(self.held);
        }
        if on {
            self.hold(why)?
        } else {
            self.release()
        }
        self.held = on;
        Ok(on)
    }

    /// Whether it is held now.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Whether the hold still stands: on macOS and Linux the helper may have ended (no logind on
    /// this Linux, say), and then it is let go. Call it now and then.
    pub fn still_held(&mut self) -> bool {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if self.held && self.child.as_mut().is_none_or(|c| !matches!(c.try_wait(), Ok(None))) {
            self.child = None;
            self.held = false;
        }
        self.held
    }

    #[cfg(windows)]
    fn hold(&mut self, _why: &str) -> Result<(), String> {
        if ffi::hold_awake(true) { Ok(()) } else { Err("Windows refused to keep the PC awake".into()) }
    }

    #[cfg(windows)]
    fn release(&mut self) {
        ffi::hold_awake(false);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn hold(&mut self, why: &str) -> Result<(), String> {
        use std::process::{Command, Stdio};
        let pid = std::process::id().to_string();
        let mut cmd = if cfg!(target_os = "macos") {
            let mut c = Command::new("caffeinate");
            c.args(["-i", "-w", &pid]);
            c
        } else {
            let mut c = Command::new("systemd-inhibit");
            c.args([
                "--what=idle:sleep",
                "--who=FreeDAWm",
                &format!("--why={why}"),
                "--mode=block",
                "tail",
                &format!("--pid={pid}"),
                "-f",
                "/dev/null",
            ]);
            c
        };
        let child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| {
            if cfg!(target_os = "macos") { format!("caffeinate: {e}") } else { format!("cannot keep this PC awake (systemd-inhibit: {e})") }
        })?;
        self.child = Some(child);
        Ok(())
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn release(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    fn hold(&mut self, _why: &str) -> Result<(), String> {
        Err("keeping the computer awake is not supported on this system".into())
    }

    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    fn release(&mut self) {}
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        if self.held {
            self.release();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keep_awake_holds_and_releases_or_says_why_not() {
        let mut k = super::KeepAwake::default();
        match k.set(true, "test") {
            Ok(held) => {
                assert!(held && k.is_held());
                assert_eq!(k.set(true, "test"), Ok(true), "holding twice is one hold");
                assert_eq!(k.set(false, "test"), Ok(false));
                assert!(!k.is_held());
            }
            // No systemd (a container): said in words, nothing held.
            Err(e) => {
                assert!(!e.is_empty() && !k.is_held());
            }
        }
    }

    #[test]
    fn the_time_zone_offset_is_plausible() {
        if cfg!(any(windows, target_os = "macos", target_os = "linux")) {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap();
            let o = super::utc_offset_secs(now).unwrap();
            assert!(o.abs() <= 14 * 3600 && o % 60 == 0, "{o}");
        }
    }

    #[test]
    fn the_memory_of_a_desktop_platform_is_known_and_plausible() {
        if cfg!(any(windows, target_os = "macos", target_os = "linux")) {
            let m = super::total_memory();
            assert!(m.is_some_and(|b| b >= 256 << 20), "{m:?}");
        }
    }

    #[test]
    fn the_free_space_of_the_temp_folder_is_known() {
        if cfg!(any(unix, windows)) {
            assert!(super::free_disk_space(&std::env::temp_dir()).is_some());
        }
        assert_eq!(super::free_disk_space(std::path::Path::new("/no/such/folder/anywhere")), None);
    }
}
