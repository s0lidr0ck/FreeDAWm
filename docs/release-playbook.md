# Release playbook

> **Last updated:** 2026-11-10 · **Change:** minor (song engines in `models-v1`: `engines.json`, the Song
> engines workflow; previously minor: the ML models release `models-v1`, the free release workflow,
> the export's sync mode)

The release playbook shared by the crafting apps lives in craftrules
(`release/playbook.md`); FreeDAWm-specific packaging details are in [releasing.md](releasing.md).
This page covers what FreeDAWm needs on top: publishing the source as a public repository,
CI, and the licence questions a binary release raises.

## Status

- **CI:** `.github/workflows/ci.yml` runs `cargo xtask ci` (Linux) and `cargo test --workspace`
  (macOS, Windows) on every pull request and on pushes to `main`, with a pinned toolchain. It uses
  no secrets.
- **Release workflow:** `.github/workflows/release.yml` builds unsigned packages on the free
  runners (Windows x64 with ASIO under GPLv3, see below; macOS Apple silicon, Linux x86_64) and publishes them on a
  `v*` tag (a draft release on a manual run). See [releasing.md](releasing.md). The upstream
  FreeBSD, Nix, packaging-lint and Windows-Arm workflows were not restored.
- **Dependabot** (`.github/dependabot.yml`) groups minor and patch updates and ignores Cargo
  semver-major ones (they need code changes: do them by hand).

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
For later changes, **sync** into a clone of the public repository instead:

```sh
git clone https://github.com/s0lidr0ck/FreeDAWm D:/src/FreeDAWm   # once
python tools/public-export/export.py --sync D:/src/FreeDAWm --ref main -m "What changed"
```

The same export, exclusions and scan run (the commit message is scanned too: no agent trailers,
session links or private e-mail addresses), then the clone's tracked files are replaced by the
exported tree (its `.git` and ignored files stay, executable bits follow the private commit) and
one ordinary commit is made on the clone's current branch, author and committer the identity of
its last commit unless `--author-name` / `--author-email` say otherwise. The clone must be clean;
nothing is pushed. If the public repository got commits of its own (a README edit, a merged
pull request), merge them into the private branch and the public branch first, or the sync
reverts them.

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

Source releases and default builds are unaffected. Decided 2026-11-04 (owner): the GitHub Windows
release builds use ASIO under **GPLv3**. `release.yml` downloads
`ASIO-SDK_2.3.4_2025-10-15.zip` from download.steinberg.net, checks its SHA-256 and that its
`LICENSE.txt` still offers GPLv3, and runs `packaging/windows/package.ps1 -Asio`, which adds
`GPL-3.0.txt` and `ASIO-NOTICE.txt` (source offer: the public repo at the release tag) to the MSI
and the portable zip. The SDK is never committed. No ASIO logo is shown, so the ASIO usage
guidelines' logo rules don't apply; "ASIO" appears only as a driver type, never in the product name.
A new SDK version means updating the URL and SHA-256 in `release.yml`.

Third-party ML models are not bundled. Setup › ML Models downloads them, after a click, from our own
`models-v1` release (below) and shows each model's licence; models that are non-commercial or state
no licence ask for a confirm first ("personal use only").

## ML models release (`models-v1`)

The converted (ONNX) models are assets of one GitHub release on the public repo, tag
**`models-v1`** on `s0lidr0ck/FreeDAWm`, not tied to app versions (spec
`docs/superpowers/specs/2026-11-04-model-downloads-design.md`). The app reads
`https://github.com/s0lidr0ck/FreeDAWm/releases/download/models-v1/models.json`
(`FREEDAWM_MODELS_URL` overrides it). Upload only with the owner's go-ahead.

1. Pack the installed models (each file is checked against its card first; `golden/` test data is
   left out; a model over GitHub's 2 GiB asset limit, or not installed, is listed as "install
   manually" in the manifest, as `whisper_lyrics` is at 4.75 GB):

   ```sh
   cargo xtask ml-publish --models "$LOCALAPPDATA/SoundCraft/models" --out ../models-publish
   ```

   It writes `<id>.zip` per model, `models.json` (id, version = the card's `exported_utc`, url,
   size, sha256, licence status and note, source, `personal_use_only`, or `manual` and
   `manual_note`) and `MODELS-NOTICE.md` (every model, source and licence).
2. Create the release once (not marked latest, so it never looks like an app release), with the
   notice as its notes:

   ```sh
   cd ../models-publish
   gh release create models-v1 --repo s0lidr0ck/FreeDAWm --title "FreeDAWm models (v1)" \
     --notes-file MODELS-NOTICE.md --latest=false
   ```
3. Upload the zips first and the manifest last, so `models.json` never names a missing asset:

   ```sh
   gh release upload models-v1 --repo s0lidr0ck/FreeDAWm --clobber *.zip
   gh release upload models-v1 --repo s0lidr0ck/FreeDAWm --clobber models.json MODELS-NOTICE.md
   ```
4. Check: `curl -sL https://github.com/s0lidr0ck/FreeDAWm/releases/download/models-v1/models.json | head`;
   then Setup › ML Models lists the sizes and Download buttons.

To add or update a model: install the export, then pack just that model
(`cargo xtask ml-publish … <id>`). Given ids, `ml-publish` downloads the release's current
`models.json` with `gh` (or reads `--merge FILE`) and writes it back with only those entries
replaced or added, so the manifest and notice it uploads keep every other model (`--no-merge`
writes only the given models: never upload that manifest). An updated model's `version` rises
with the card's `exported_utc`, so installed copies show **Update**. Then step 3 for that zip and
the manifest, as the command it prints.

### Song engines (`engines.json`)

Generate Song's engines (YuE2, ACE-Step 1.5) ship in the same release, from their own workflow, so
nobody builds or starts a server by hand. Each engine is one Setup › ML Models item: a zip of the
upstream server (yue2.cpp's `yue-server`, acestep.cpp's `ace-server`, MIT) built for the person's
platform, plus its GGUF models, which the app fetches from Hugging Face at the revisions pinned in
`tools/engines/engines.json` (each checked against its size and sha256). FreeDAWm starts the server
itself on a free local port when a song needs it and stops it when idle or on quit
(`crates/ml/src/engine_server.rs`). `ml-publish` leaves the engines out: they are in
`engines.json`, which the app reads beside `models.json`.

1. To change an engine (upstream commit, models, arguments), edit `tools/engines/engines.json` and
   raise its `version` (installed copies then show **Update**).
2. Actions › **Song engines** › Run workflow, with **publish** ticked (the owner's go-ahead). It
   builds both servers for `linux-x86_64` (Vulkan with every CPU variant as loadable backends,
   libraries found beside the program), `windows-x86_64` (the same, Vulkan SDK from LunarG) and
   `macos-aarch64` (one static program with Metal), checks each one starts, packs them
   (`tools/engines/package.py`: `bin/`, `card.json`, `LICENSE-server`), writes `engines.json`
   (`tools/engines/manifest.py`) and uploads the zips and `engines.json` to `models-v1`.
   Without **publish** (and on pull requests touching `tools/engines/`) it only builds and packs.
3. Check: `curl -sL https://github.com/s0lidr0ck/FreeDAWm/releases/download/models-v1/engines.json | head`;
   Setup › ML Models lists "YuE2 song engine" and "ACE-Step 1.5 song engine" with Download (about
   4.4 GB each, nearly all of it the models). Try a song on Windows and macOS before announcing it:
   the CI checks that each program starts, not that it sings on real GPUs.
