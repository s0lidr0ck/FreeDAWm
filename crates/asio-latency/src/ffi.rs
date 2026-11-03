//! The FFI: the one `unsafe` call, into `src/shim.cpp`.

use std::ffi::c_long;

unsafe extern "C" {
    /// `ASIOGetLatencies` through the shim: input and output latency in device frames, and the
    /// ASIO error code. Null-safe; with no driver loaded it writes 0/0 and returns ASE_NotPresent.
    fn sc_asio_get_latencies(input: *mut c_long, output: *mut c_long) -> c_long;
}

/// The ASIO error code and the input and output latencies the loaded driver reports.
pub(crate) fn get_latencies() -> (i32, i32, i32) {
    let (mut input, mut output): (c_long, c_long) = (0, 0);
    // SAFETY: both pointers come from live, aligned, writable locals that outlive the call, and
    // the shim writes only one `long` through each. `ASIOGetLatencies` reads the SDK's global
    // driver pointer, which is null (ASE_NotPresent) or the driver asio-sys loaded and keeps
    // alive while cpal's streams exist; the shim catches C++ exceptions so none unwind into Rust.
    let code = unsafe { sc_asio_get_latencies(&mut input, &mut output) };
    (code, input, output)
}
