//! `freedawm-plugin-host`: runs one hosted plug-in for the app (`soundcraft-plugin-bridge`),
//! its audio over shared memory and its requests on stdin. A plug-in that crashes here only
//! costs this process: the app silences that plug-in and carries on.

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(u8::try_from(soundcraft_plugin_bridge::serve()).unwrap_or(1))
}
