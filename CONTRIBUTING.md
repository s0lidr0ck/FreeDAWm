# Contributing to FreeDAWm

Thanks for helping. FreeDAWm is a pure-Rust digital audio workstation (egui UI) with a familiar
Edit/Mix workflow. This page is what you need to build it, change it and get a pull request
merged.

## Build and run

- Rust stable (1.95 or later for the desktop and web apps; the core crates build from 1.90).
- Linux: `pkg-config`, the ALSA, PipeWire and JACK headers (`libasound2-dev libpipewire-0.3-dev
  libjack-jackd2-dev` / `alsa-lib-devel pipewire-devel pipewire-jack-audio-connection-kit-devel`),
  OpenSSL headers,
  and the X11/Wayland development packages your distribution's egui/winit guide lists.
- Transcribe Lyrics builds whisper.cpp from source: CMake on `PATH` (or `CMAKE` pointing at it)
  and libclang for bindgen (`LIBCLANG_PATH` if it is not found). The first build takes about a
  minute.

```sh
cargo run --release -p soundcraft -- --demo     # the app (`freedawm`) with the demo song
cargo run -p soundcraft-cli -- --help           # the headless CLI (`freedawm-cli`)
cd apps/soundcraft-web && trunk serve --release # the web build
```

The crates and cargo packages keep their `soundcraft-*` names (FreeDAWm is a fork of SoundCraft);
the programs, settings folders and `FREEDAWM_*` environment variables are FreeDAWm's.

The optional `asio` feature (Windows) needs the Steinberg ASIO SDK, which is not part of this
repository; a binary built with it carries the SDK's licence terms (see
`docs/release-playbook.md`). Default builds do not use it.

## Before you open a pull request

`cargo xtask ci` must pass. It runs, in order: `cargo fmt --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace`, the asset check, the layering check and
the WebAssembly check. CI runs the same command on every pull request.

While you work, test the crates you touch: `cargo test -p <crate> <filter>`. Some tests print
`SKIPPED` when local-only reference audio is missing; that is expected.

One topic per pull request, with tests for the behaviour you add or fix.

## The rules that keep FreeDAWm trustworthy

**Never crash.** People trust a DAW with their recordings.

- No panics in non-test code: no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!` or
  `unimplemented!`. Errors are `Result` values and `?`. Every production crate root denies these
  with clippy lints.
- No `unsafe`, except in the isolated FFI helper crates (`crates/clap-host`, `crates/vst3-host`,
  `crates/au-host`, `crates/asio-latency`), where each block carries a `// SAFETY:` comment.
- Input-derived numbers (files, commands, control/MCP parameters) are hostile: use `get()` rather
  than indexing, checked or saturating arithmetic, cap allocations.
- The audio callback never blocks, never allocates in steady state and never panics.
- A crash fix lands with a regression test.

**Assets.** Every bundled image, icon, sound, font or preset must be original, public domain or
under a permissive licence, and gets a row in [`ATTRIBUTION.md`](ATTRIBUTION.md) (path, author,
source, licence) in the same commit; `cargo xtask assets` enforces it. Never copy resources out of
another product (icons, artwork, presets, sounds, text). No GPL/AGPL/LGPL code.

**Trademarks.** Other products are named only to describe compatibility or comparison, as
[`NOTICE`](NOTICE) explains. Modes and presets named after hardware carry a credit line
(`crates/ui-egui/src/hardware_credit.rs`).

## How the code fits together

- Layers (enforced by `cargo xtask layers`): time/audio-io/midi → dsp → model → mix →
  engine/playback → automation → ui-egui → apps. Nothing below the UI depends on egui.
- **Everything is a command.** User-visible behaviour is a command in `crates/engine/src/cmd/*`
  (id, label, menu path, shortcut, parameters, `enabled`, `run`) with tests; UI-only commands live
  in `crates/ui-egui/src/menus.rs`. Menus, shortcuts, the CLI, the control channel and MCP all
  dispatch the same ids. A new command goes in its own module with its own `specs()`.
- The menu bar as shown is `crates/ui-egui/src/menubar.txt`; tests in
  `crates/ui-egui/tests/menu_bar.rs` pin its rows.
- Drive the running app over the control channel (`--control 0`, JSON lines over TCP; see
  `docs/control-protocol.md`) or headless with `freedawm-cli`; render the UI offscreen with
  `cargo run -p soundcraft-ui-egui --example ui_shot -- out.png`.

More detail: [`docs/architecture.md`](docs/architecture.md), [`ROADMAP.md`](ROADMAP.md) and
[`docs/gaps.md`](docs/gaps.md) (what to work on).

## Licence of contributions

FreeDAWm is dual-licensed under MIT or Apache-2.0. Unless you say otherwise, any contribution you
submit is licensed under the same terms (inbound = outbound), without additional conditions.

## Credits

The About box credits contributors from `contributors/`. If you want to be credited by more than
your GitHub username, add only your own entry to `contributors/people.toml` in your pull request
(see `docs/contributors.md`). Never add or edit anyone else's entry.
