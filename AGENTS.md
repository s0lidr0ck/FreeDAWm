# FreeDAWm — instructions for agents

FreeDAWm is an open-source, pure-Rust digital audio workstation with a familiar Edit/Mix
workflow. It runs natively on macOS, Windows, Linux and FreeBSD, and in the browser via WASM.

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) first: build, `cargo xtask ci`, the never-crash and asset
rules, and how commands, menus and tests fit together all apply to agents too. The short version:

- **Gate:** `cargo xtask ci` (fmt, clippy `-D warnings`, tests, assets, layers, wasm) passes before
  every commit; while working, test the crates you touch (`cargo test -p <crate> <filter>`).
- **Never crash:** no `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!`/`unimplemented!` in
  non-test code, no `unsafe` outside the FFI helper crates, `Result` + `?`, hostile input handled
  with `get()` and checked arithmetic, an audio callback that never blocks, allocates or panics.
- **Everything is a command:** user-visible behaviour is a command in `crates/engine/src/cmd/*`
  (UI-only ones in `crates/ui-egui/src/menus.rs`); the menu bar as shown is
  `crates/ui-egui/src/menubar.txt`.
- **Assets:** original or permissively licensed only, each with an `ATTRIBUTION.md` row in the same
  commit. Never copy resources out of another product.
- **Names:** the crates keep their `soundcraft-*` names; the programs, settings folders and
  `FREEDAWM_*` environment variables are FreeDAWm's.
- **Look at UI changes:** `cargo run -p soundcraft-ui-egui --example ui_shot -- out.png [script.jsonl]`
  renders offscreen; `cargo run --release -p soundcraft -- --demo --control 0` runs the app with a
  control channel (`docs/control-protocol.md`).
- Parallel agents: one `CARGO_TARGET_DIR` per agent; edit only what you own.
- Contributor credits: add only your own human's entry to `contributors/people.toml`, and only if
  they ask (see `docs/contributors.md`).
