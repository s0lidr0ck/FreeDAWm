//! `freedawm-scan --format vst3|clap --out <results file> <bundle>...`: loads each bundle in
//! this process, in order, and reports its plugins (or why it would not load) to the results file
//! one JSON line at a time, then a `done` line. The app runs it over a few bundles per process
//! (`soundcraft-plugin-scan`): a plugin that crashes or hangs here only costs this process.

use soundcraft_plugin_scan::{ResultWriter, parse_helper_args};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = match parse_helper_args(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    match scan(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("freedawm-scan: {}: {e}", args.out.display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn scan(args: &soundcraft_plugin_scan::HelperArgs) -> std::io::Result<()> {
    // No crash dialog may wait for a click: the app stops a helper that goes quiet.
    soundcraft_vst3_host::quiet_crash_dialogs();
    let mut out = ResultWriter::create(&args.out)?;
    for b in &args.bundles {
        if args.format == "clap" {
            out.bundle(b, soundcraft_clap_host::load_bundle(b).map_err(|e| e.to_string()))?;
        } else {
            out.bundle(b, soundcraft_vst3_host::load_bundle(b).map_err(|e| e.to_string()))?;
        }
    }
    out.done()
}

#[cfg(target_arch = "wasm32")]
fn scan(args: &soundcraft_plugin_scan::HelperArgs) -> std::io::Result<()> {
    ResultWriter::create(&args.out)?.done()
}
