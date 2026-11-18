//! The computer's physical memory, so the disk cache (`soundcraft_model::disk`) can size itself
//! to the machine: generous on a big PC, never pushing a small one into paging; and the free
//! space on a disk, so recording can show how long it can go on.
//!
//! This is an isolated `unsafe` helper crate (craftrules never-crash): `unsafe` is denied
//! crate-wide and allowed only in the private `ffi` module (one OS call per platform, each with a
//! `// SAFETY:` comment). The public API is safe and never panics.

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

#[cfg(test)]
mod tests {
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
