//! The OS calls (the only `unsafe` in the crate).

#[cfg(windows)]
pub(crate) fn total_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).ok()?,
        dwMemoryLoad: 0,
        ullTotalPhys: 0,
        ullAvailPhys: 0,
        ullTotalPageFile: 0,
        ullAvailPageFile: 0,
        ullTotalVirtual: 0,
        ullAvailVirtual: 0,
        ullAvailExtendedVirtual: 0,
    };
    // SAFETY: `status` is a valid, writable MEMORYSTATUSEX whose `dwLength` is set to its size,
    // as GlobalMemoryStatusEx requires; it only writes into that struct.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some(status.ullTotalPhys)
}

#[cfg(target_os = "macos")]
pub(crate) fn total_memory() -> Option<u64> {
    let mut bytes: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: the name is a NUL-terminated C string; `bytes` is a writable u64 and `len` holds its
    // size, which is what `hw.memsize` returns; no new value is set (null, 0).
    let rc = unsafe { libc::sysctlbyname(c"hw.memsize".as_ptr(), (&raw mut bytes).cast(), &raw mut len, std::ptr::null_mut(), 0) };
    (rc == 0 && len == std::mem::size_of::<u64>()).then_some(bytes)
}

#[cfg(target_os = "linux")]
pub(crate) fn total_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kb.checked_mul(1024)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub(crate) fn total_memory() -> Option<u64> {
    None
}

/// Bytes free for this user on the file system holding `path` (`statvfs`'s `f_bavail` blocks).
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // the field types differ between platforms
pub(crate) fn free_disk_space(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` is a NUL-terminated path that lives across the call and `st` is writable
    // memory of the size statvfs fills; it writes only into `st`.
    let rc = unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: statvfs returned 0, so it filled `st` (and it was zeroed before).
    let st = unsafe { st.assume_init() };
    (st.f_bavail as u64).checked_mul(st.f_frsize as u64)
}

/// Bytes free for this user on the volume holding `path` (`GetDiskFreeSpaceExW`).
#[cfg(windows)]
pub(crate) fn free_disk_space(path: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free: u64 = 0;
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that lives across the call; `free` is a
    // writable u64; the two other outputs are optional and passed as null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn free_disk_space(_path: &std::path::Path) -> Option<u64> {
    None
}
