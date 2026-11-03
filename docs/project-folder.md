# What a project folder holds

Last updated: 2026-11-02 (spec `docs/superpowers/specs/2026-11-02-project-folder-split-songs-follower-display.md`, section 2;
generated audio on disk and its subfolders: spec `docs/superpowers/specs/2026-11-02-disk-streaming-design.md`).

A saved session is a folder: everything the session needs to open and sound the same lives in
it, so the folder can be copied, backed up or moved to another computer as it is. The app's own
settings folder (`%APPDATA%\FreeDAWm` on Windows, "the config dir" below) holds only this
computer's settings, libraries shared by every project, and crash-recovery copies.

```
<Project>/
  <Project>.fdawm          the session (JSON): tracks, clips, mix, automation, markers, lyrics,
                           MIDI, vocal tuning edits, Scout captures, plugin states
  Audio Files/             every audio file the session plays (see the table): recordings and
                           imports at the top, generated audio in subfolders:
    Stems/                 Separate Stems, Karaoke, Extract Drums
    Clean Vocal/           Clean Vocal's cleaned audio
    Tuning/                vocal tuning renders (Commit)
    Freeze/                Freeze and Commit renders
    Processed/             Process (AudioSuite), Consolidate, clip gain renders, Convert/Tempo renders
    Split/                 Split into Files pieces
  Waveforms/               waveform cache, <audio file>.peaks (safe to delete; made again)
  Remote Takes/            on a piano or guitar PC following a booth: each take recorded for the booth
                           (and an audio take's FLAC transfer file until it landed); on the booth:
                           remote audio transfer files as they arrive
  Bounced Files/           where Bounce Mix offers to write (a deliverable; the session doesn't need it)
  Backups/                 the last ten versions of the session file
```

## Audit: what lives where

| What | Before (2026-11-01) | Now |
|---|---|---|
| Recorded audio (saved session) | `Audio Files/` (written in place while recording) | same |
| Recorded audio (session never saved) | autosave folder, copied into `Audio Files/` at the first save | same (spec: fine) |
| Imported audio | copied into `Audio Files/` on save (re-encoded as 32-bit float WAV), no way to keep a reference | same by default; **Preferences › Operation › "Copy imported audio into the project"** (on) can be turned off to keep a file imported at the session rate as a reference. The original is never moved or deleted |
| Sampler samples | `Audio Files/` on save (in memory until then) | same |
| Stems (Separate Stems), Clean Vocal, vocal tuning renders, Freeze, Commit, Consolidate, Process, clip gain renders, Convert/Tempo renders, Split into Files | in memory, written to `Audio Files/` on save | **written when made** (2026-11-02, disk streaming) into the subfolder of its kind (`Audio Files/Stems/`, …, see the tree), then played from disk; a session never saved writes them under its autosave folder (`<config dir>/Autosave/<session name>/Audio Files/<kind>/`, beside its takes) and the first save copies them into the project's subfolders. Older sessions keep their files where they are (nothing is moved on open). Generated files that nothing names are deleted when the session is saved: not the session, its undo/redo steps, any other session file in the project folder, any backup in `Backups/` or any autosave (only inside the generated subfolders, never recordings or imports; when one of those files can't be read, nothing is deleted) |
| DAWproject imports | embedded audio decoded into memory, written to `Audio Files/` on save; referenced audio copied the same way | same |
| Video (Import Video) | the movie referenced where it is (its audio track copied into `Audio Files/`) | same: **the one reference left**, by choice (movies are large; Pro Tools also links video). A moved movie shows as missing media and can be relinked |
| Module files: Amp cabinet IR from a file, LevelUp reference tracks | **referenced by absolute path outside the project** (a moved file went silent) | copied into `Audio Files/` on save (with the copy preference on) and named from there; bundled IRs stay `bundled:` |
| Waveforms (peak overviews) | rebuilt from the audio on every open | cached in `Waveforms/` (keyed by file name, size, modified time and session rate); read on open, made again if missing or stale, written on save. Never-saved sessions keep them in memory |
| Bounce Mix / Export Stems / Export MIDI / clip exports | wherever the person picks; Bounce offered the project folder's root | Bounce offers `<project>/Bounced Files/` (made when needed). Deliverables, not needed by the session |
| Session backups | `<project>/Backups/` | same |
| Autosave / crash recovery of a never-saved session | config dir `Autosave/` | same (spec: fine) |
| Autosave of a saved session | config dir `Autosave/` (a recovery copy; the project itself is untouched) | same |
| Piano PC (follower) takes waiting to be sent | config dir `Remote Outbox/<take>.json` (the take itself) | `<project>/Remote Takes/<take>.json` (kept after it lands); the outbox keeps only a pointer `<take>.ptr`. Old outbox files are still read and sent |
| Piano PC session never saved | config dir `Remote Takes/<PC> – <booth session> – <date>.fdawm` (loose file) | its own project folder `Documents/FreeDAWm/Remote Takes/<PC> – <booth session> – <date>/` (Setup › Network › Follow a booth can change the parent folder) |
| Booth: remote takes landed | MIDI clips in the booth's session | same; remote **audio** takes land as `Audio Files/<PC> <input>_NN.wav` at the session's rate and bit depth, their transfer files in `<project>/Remote Takes/` (a never-saved booth session: `<config dir>/Remote Inbox/`) |
| Guitar PC (follower) audio takes | — | the take's WAV in its project's `Audio Files/` (as any take); its FLAC transfer file in `<project>/Remote Takes/` until it landed |
| Lyrics, transcriptions, Scout captures and answers, MIDI, tuning edits, markers, plugin states | in the session file | same |
| Scout sound profiles | config dir `profiles/` | same: a library shared by every project, not session data |
| ML models, plugin scan cache, plugin pictures, presets, Band library, shortcuts, `ui.json`, `network.json`, recent files | config dir | same: this computer's, not session data |

## Audio played from disk

Opening a session reads each audio file's header only: a native PCM/float WAV at the session's
rate plays straight from disk through one shared cache of decoded audio (`soundcraft_model::disk`),
sized to a quarter of the computer's memory, at least 512 MiB and at most 6 GiB (`FREEDAWM_DISK_CACHE_MB` overrides; `FREEDAWM_STREAM=0` decodes
every file into memory as before). A session that fits in the cache ends up wholly cached a few
seconds after opening and plays as from memory; a longer one keeps what is around the playhead
(read ahead about four seconds, the loop start too). Other files (compressed formats, another
rate) and samples a sampler plays are decoded into memory as before. Waveforms come from
`Waveforms/` (made by reading the file through once when missing). Imported audio and audio
made before the first save play from their files once saved.

## Save As and Save Session Copy In

Both write a complete project into the new folder: every audio file the session plays (also
those of the folder it was saved in before, and those it referred to elsewhere) and the module
files, copied into the new `Audio Files/`, and the waveform cache. With "Copy imported audio into
the project" off, references to files outside any project stay references.
