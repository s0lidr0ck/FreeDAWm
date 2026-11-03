# Release playbook

> **Last updated:** 2026-11-02 · **Change:** minor (public export, CI, ASIO licensing, the
> placeholder repository URL)

The release playbook shared by the crafting apps lives in craftrules
(`release/playbook.md`); FreeDAWm-specific packaging details are in [releasing.md](releasing.md).
This page covers what FreeDAWm needs on top: publishing the source as a public repository,
CI, and the licence questions a binary release raises.

## Status

- **CI:** `.github/workflows/ci.yml` runs `cargo xtask ci` on every pull request and on pushes to
  `main` (Linux). It uses no secrets.
- **Release workflow:** not in the repository. The upstream `release.yml` (and the FreeBSD, Nix,
  packaging-lint and Windows-Arm workflows) were removed when the fork went private; restore them
  from upstream only after the signing secrets and the ASIO decision below are settled.
  `packaging/` and [releasing.md](releasing.md) still describe them.

## Public export (fresh history)

The private repository keeps its own history; it is never rewritten or pushed to the public
repository. The public repository starts from a cleaned snapshot with one initial commit, made by
`tools/public-export/export.py` (Python 3.9+, standard library only):

```sh
# from the private checkout; the destination must not exist (or be empty)
python tools/public-export/export.py --dest D:/out/freedawm-public
python tools/public-export/export.py --dest D:/out/freedawm-public --ref main \
    --author-name "Your Name" --author-email "12345+you@users.noreply.github.com"
```

What it does:

1. **Reads one committed tree** (`--ref`, default `HEAD`) with `git archive`: uncommitted changes
   and untracked files are never exported.
2. **Leaves out the private material** listed in `tools/public-export/exclude.txt`:
   `docs/superpowers/` (design notes, plans, ledgers), `plan/`, `docs/open-source-audit.md`,
   the generated `docs/parity-checklist.md`, the private `AGENTS.md` and `CLAUDE.md`, and the
   export tool itself. `crates/engine/catalog/menus.txt` stays because the engine compiles it in
   (feature names only).
3. **Writes the public agent instructions** (`tools/public-export/AGENTS.public.md` as
   `AGENTS.md`, and a one-line `CLAUDE.md` pointing at it). Contributors read `CONTRIBUTING.md`.
4. **Scans every text file** for secrets (API-key shapes, private keys, password assignments),
   personal paths (Windows/macOS/Linux user folders), Claude session links, e-mail addresses
   other than GitHub noreply / `example.com` ones, and private terms: every author and committer
   name and e-mail in the private history, this machine's user name and home folder, plus
   `--private-term WORD` and an optional `tools/public-export/private-terms.txt` (one per line).
   Any finding stops the export before `git init` (exit 1) and lists `path:line: what`; fix it in
   the private repository, or add a narrow, reviewed regex to `tools/public-export/allow.txt`.
5. **Makes the repository:** `git init -b main` in the destination and one commit, author and
   committer both the chosen identity (default `FreeDAWm contributors
   <freedawm@users.noreply.github.com>`, a placeholder: use your own GitHub noreply address, or
   set `FREEDAWM_EXPORT_NAME` / `FREEDAWM_EXPORT_EMAIL`). It adds **no remote** and pushes
   nothing.

Other options: `--no-git` (write and scan only), `--branch`, `--message`, `--source` (another
checkout), `--skip-scan` (not recommended).

Then, in the destination: run `cargo xtask ci`, read the tree once more (README, NOTICE,
ATTRIBUTION), create the public GitHub repository, add it as the remote yourself and push.
For later releases, export again into a fresh folder and copy the changes over in the public
repository as ordinary commits (or make the public repository the main one).

## Before the first public push

- **Repository URL.** The tree points at the placeholder `https://github.com/s0lidr0ck/FreeDAWm`
  (and the app id `io.github.s0lidr0ck.FreeDAWm`, which follows it). Once the real repository
  exists, replace the placeholder everywhere: `Cargo.toml` (`repository`), `README.md`,
  `crates/ui-egui/src/brand_art.rs` (`REPOSITORY_URL`), `crates/clap-host/src/ffi.rs` (the host
  URL), `packaging/linux/io.github.s0lidr0ck.FreeDAWm.metainfo.xml.in`, `packaging/linux/nfpm.yaml`,
  `packaging/linux/package.sh`, `packaging/windows/freedawm.wxs`, `packaging/windows/sign.ps1` and
  `docs/releasing.md`. If the GitHub owner is not `freedawm`, also rename the app id (the
  `packaging/linux/io.github.s0lidr0ck.FreeDAWm.*` files, `assets/app-icon/hicolor/*/apps/`,
  `packaging/icons.py`, `packaging/macos/Info.plist.in`, the window app id in
  `apps/soundcraft/src/main.rs` and `splash.rs`); settings folders are found by name, so the
  rename does not affect them.
- **Name check:** trademark registers, the GitHub organisation and a domain for "FreeDAWm".
- **Owner's open decisions:** see the "left for the owner" list in the latest public-cleanup
  report (band players named after real musicians, the ML models' licences, the remaining
  hardware-named engines outside the M3 list).

## ASIO (Windows) and binary licences

The `asio` feature is off by default and no CI builds it. Since 2025-10-29 Steinberg offers the
ASIO SDK under GPLv3 as well as its proprietary licence agreement. A binary built with
`--features asio` contains SDK code, so it can be distributed only:

- as **GPLv3** (the MIT/Apache sources are compatible; the combined binary is GPLv3), with the
  corresponding source offer; or
- under **Steinberg's ASIO licence agreement** (signed with Steinberg).

Source releases and default builds are unaffected. Decide this before publishing any Windows
binary with ASIO, and say which in its release notes.

Third-party ML models are not bundled; the app downloads them from their authors and shows each
model's licence. Some are non-commercial or state no licence: keep them opt-in in public builds.
