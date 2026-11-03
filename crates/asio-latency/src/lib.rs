//! The ASIO driver's own latency figures (`ASIOGetLatencies`), which include its converter and
//! safety offsets, for `soundcraft-playback`'s reported latency on ASIO devices.
//!
//! asio-sys (cpal's ASIO backend) leaves `ASIOGetLatencies` out of its bindings, but the SDK
//! library it builds and links has it. A tiny C++ shim (`src/shim.cpp`, built by `build.rs`)
//! calls it; it links against that same library, so it reads the very driver cpal loaded (there
//! is one copy of the SDK's driver state).
//!
//! This is an isolated `unsafe` helper crate (craftrules never-crash): `unsafe` is denied
//! crate-wide and allowed only in the private `ffi` module. The public API is safe, returns
//! `Result` and never panics.
//!
//! Without the `asio` feature, or off Windows, it is a stub whose answer is always an error.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(all(feature = "asio", windows))]
#[allow(unsafe_code)]
mod ffi;

// Links the SDK library (`asio.lib`) the shim calls into: the one cpal's ASIO host uses.
#[cfg(all(feature = "asio", windows))]
use asio_sys as _;

/// `ASE_OK`.
const ASE_OK: i32 = 0;
/// `ASE_NotPresent`: no driver loaded.
const ASE_NOT_PRESENT: i32 = -1000;

/// The loaded ASIO driver's input and output latency, in device frames.
///
/// Valid only while the driver's buffers exist: call it on the UI thread after cpal's ASIO
/// streams have started, never on the audio thread. An error when no driver is loaded, the
/// driver refuses, a figure is not positive, or ASIO support isn't built in.
pub fn driver_latencies() -> Result<(u32, u32), String> {
    #[cfg(all(feature = "asio", windows))]
    {
        let (code, input, output) = ffi::get_latencies();
        interpret(code, input, output)
    }
    #[cfg(not(all(feature = "asio", windows)))]
    {
        Err("ASIO support is not built in".into())
    }
}

/// `ASIOGetLatencies`'s answer: `ASE_OK` with two positive figures, else an error.
#[cfg_attr(not(all(feature = "asio", windows)), allow(dead_code))]
fn interpret(code: i32, input: i32, output: i32) -> Result<(u32, u32), String> {
    match code {
        ASE_OK => match (u32::try_from(input), u32::try_from(output)) {
            (Ok(i), Ok(o)) if i > 0 && o > 0 => Ok((i, o)),
            _ => Err(format!("the ASIO driver reported unusable latencies ({input} in, {output} out)")),
        },
        ASE_NOT_PRESENT => Err("no ASIO driver is loaded".into()),
        e => Err(format!("the ASIO driver refused its latencies (error {e})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ase_ok_with_positive_figures_is_a_latency() {
        assert_eq!(interpret(ASE_OK, 96, 160), Ok((96, 160)));
        assert!(interpret(ASE_NOT_PRESENT, 0, 0).is_err());
        assert!(interpret(-999, 96, 160).is_err());
        assert!(interpret(0x3f48_47a0, 96, 160).is_err(), "ASE_SUCCESS is not ASE_OK");
        assert!(interpret(ASE_OK, 0, 160).is_err());
        assert!(interpret(ASE_OK, 96, -1).is_err());
        assert!(interpret(ASE_OK, i32::MIN, i32::MIN).is_err());
    }

    /// No driver is loaded in a test process: the real call (through the shim and the SDK
    /// library) or the stub says so, and nothing panics.
    #[test]
    fn without_a_loaded_driver_it_is_an_error() {
        assert!(driver_latencies().is_err());
    }

    /// The shim reaches asio.cpp's ASIOGetLatencies: with no driver it writes 0/0 and returns
    /// ASE_NotPresent.
    #[cfg(all(feature = "asio", windows))]
    #[test]
    fn the_shim_reaches_the_sdk_library() {
        assert_eq!(ffi::get_latencies(), (ASE_NOT_PRESENT, 0, 0));
    }
}
