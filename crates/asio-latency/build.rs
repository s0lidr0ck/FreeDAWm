//! Compiles `src/shim.cpp` (one `extern "C"` wrapper around the SDK's `ASIOGetLatencies`) against
//! the ASIO SDK headers, only for the `asio` feature on Windows. It does NOT compile the SDK's
//! `asio.cpp`: `ASIOGetLatencies` and the driver state it reads (`theAsioDriver`) come from the
//! one `asio.lib` asio-sys builds and links, so the shim sees the driver cpal loaded.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=src/shim.cpp");
    println!("cargo:rerun-if-env-changed=CPAL_ASIO_DIR");
    let asio = std::env::var_os("CARGO_FEATURE_ASIO").is_some();
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !(asio && windows) {
        return;
    }
    // The SDK asio-sys builds from. CPAL_ASIO_DIR is required (not asio-sys's temp-folder download:
    // Cargo doesn't order the two build scripts, so that copy may not exist yet).
    let Some(sdk) = std::env::var_os("CPAL_ASIO_DIR").map(PathBuf::from) else {
        println!("cargo:warning=CPAL_ASIO_DIR is not set: point it at the ASIO SDK (see `cargo xtask build-asio`)");
        std::process::exit(1);
    };
    let common = sdk.join("common");
    if !common.join("asio.h").exists() {
        println!("cargo:warning=ASIO SDK not found at {}: set CPAL_ASIO_DIR (see `cargo xtask build-asio`)", sdk.display());
        std::process::exit(1);
    }
    // `/EHsc`: the shim's `catch (...)` catches C++ exceptions only, never structured ones (an
    // access violation in a driver stays a crash, not a quiet error code).
    cc::Build::new().cpp(true).flag_if_supported("/EHsc").include(&common).file("src/shim.cpp").compile("sc_asio_latency");
}
