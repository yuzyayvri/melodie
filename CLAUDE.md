# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Melodie: a small, private, offline-first music player. One Rust binary crate.
Phases 0–3 of `PLAN.md` are implemented (library scan, playback, MPRIS/
background playback, `.m3u8` playlists, SpotiSync's Exportify-CSV sync path).
Phases 4–7 (LAN/Subsonic server, Android companion, live librespot sync,
CI hardening) are not built. Read `PLAN.md` before making architectural
changes — it's the design doc this was built from, cites its research, and
documents the tradeoffs. Read `README.md`'s "Deviations from PLAN.md"
section too; several are load-bearing bug workarounds, not style choices.

## Commands

```sh
cargo build --release        # real build: lto=fat, panic=unwind, strip
cargo build                  # debug, faster iteration
cargo test                   # all tests
cargo test db::               # one module's tests, e.g. db, engine, app, spotisync
cargo test dedupe_pending_jobs_collapses_duplicates_but_leaves_distinct_ones  # one test by name

melodie                      # opens the player window
melodie --rescan             # scan + print library, no window
melodie --repair-audio       # one-time fix for tracks downloaded with the old broken .m4a path (see below)
melodie --help
```

Building requires a C/C++ toolchain, `cmake`, and `git` (fltk-rs compiles
FLTK from source — no prebuilt/bundled option is configured). `yt-dlp` on
`PATH` is required for SpotiSync to do anything; `ffmpeg` on `PATH` is
required for SpotiSync downloads to be playable at all (see MP4/AAC note
below) — both are optional for plain playback/library use.

There is no CI, lint config, or formatter invocation configured in this repo.

## Architecture

### Threads and message passing

Four long-lived threads, wired up in `app.rs::run()`:

- **Main thread** — FLTK event loop (`fltk_app.wait()`), owns all widgets.
- **Engine thread** (`engine.rs`) — owns the `rodio::Sink`, playback queue,
  gapless look-ahead. Receives `engine::Command` over a `crossbeam_channel`,
  never touches SQLite or the filesystem except opening the audio file.
- **SpotiSync worker thread** (`spotisync/mod.rs::spawn`) — drains a `jobs`
  DB table (`match` then `download`), receives `spotisync::WorkerCommand`.
- **souvlaki's own service thread** — spawned by `mediakeys::spawn`;
  `handle_media_event` on that thread only ever touches `Sender`s, never
  FLTK widgets directly (not thread-safe to do so).

**Events flow back to the main thread through exactly one channel**:
`fltk::app::channel::<app::Message>()`. This matters because
`fltk::app::channel` is process-global and type-erased — every `Sender<T>`
in the process pushes onto the *same* underlying queue, and a `Receiver<T>`
that pops a message of the wrong type silently drops it instead of erroring.
If you're tempted to add a second `app::channel::<SomethingElse>()` call
anywhere, don't — add a variant to `app::Message` instead. `engine::Command`
and `spotisync::WorkerCommand` (the *other* direction, UI/app → worker) are
ordinary per-purpose `crossbeam_channel`s and don't have this constraint.

### Playback loop modes

`engine.rs`'s `LoopMode` (`Off`/`All`/`One`, cycled by the UI's Loop button
next to Next, persisted in `state.toml`) hooks into the *existing*
end-of-track machinery instead of adding a parallel code path — don't
reintroduce a separate "if looping" branch elsewhere if you touch this:

- **Loop-All** wraps inside `load_from`'s existing "index past the end of
  the queue" guard (already used by `Next`/`Previous`/`SetQueue`) — it just
  resets `index` to `0` there instead of falling through to `QueueEnded`.
  `tick()`'s natural end-of-queue case is routed through
  `load_from(queue.len(), true)` specifically so it shares this wrap
  instead of duplicating the check.
- **Loop-One** is intercepted in `tick()` *before* `reported_finished`
  advances — at that point `current_index()` still points at the track that
  just finished, so `load_from(idx, true)` replays it. This reuses the same
  rebuild-the-sink path `Next`/`Previous` already use rather than being
  woven into the `LOOKAHEAD` top-up logic, which stays index-sequential and
  gapless for everything else. A small gap on the loop point itself is the
  deliberate trade-off for not touching `appended`/`session_start`
  bookkeeping.

### Filesystem is the source of truth

`melodie.db` is a rebuildable cache (`db.rs::migrate` drops and recreates
all tables on any schema-version mismatch, no in-place migrations exist).
Tags live in the audio files (`lofty`); playlists live as `.m3u8` under
`_playlists/`. `playlist::sync_from_disk` is the *only* place `.m3u8` files
get turned into `playlists`/`playlist_tracks` rows, and it runs on every
startup and after every SpotiSync sync pass — playlist state always derives
from disk, never the other way except when SpotiSync itself writes a fresh
`.m3u8` (`spotisync::write_m3u8` call sites).

### SpotiSync pipeline

`_inbox/*.csv` → `exportify::parse_csv` → `spotisync::sync_playlist` upserts
`spotify_tracks`, resolves already-downloaded tracks by exact
(title, artist, album) tag match (`Db::get_track_id_by_tags` — *not* by
path; the on-disk extension isn't stable, see below), and enqueues a
`match` job for anything unresolved with no existing `matches` row. The
worker (`spotisync::spawn`'s loop) processes jobs with two things that are
easy to regress:

1. **Job kind priority, not just FIFO by id**: `Db::next_pending_job` sorts
   `download` jobs ahead of `match` jobs (`ORDER BY (kind != 'download'),
   id`). A bulk CSV import creates hundreds of `match` jobs with low ids up
   front; without this, no download would start until every match finished.
2. **No fixed per-job delay when there's backlog**: the loop only blocks on
   `recv_timeout(TICK)` when the queue is empty. A `JOB_PACING` sleep
   between jobs (not `TICK`) is the only throttle while there's a backlog —
   an earlier version waited the full idle `TICK` after *every* job
   regardless of queue depth, which made a few hundred queued tracks take
   the better part of an hour before the first download.

Matching (`matcher.rs::score_candidate`) implements the PLAN.md §5.4 scoring
table. The `RED_FLAGS` keyword list (live/cover/remix/karaoke/instrumental/
etc.) is the only thing standing between a title+duration match and
auto-downloading the wrong version of a song — it does not use NLP, just
substring checks, and has already needed one real-world addition (karaoke).
Treat additions to it as bug fixes worth a regression test, not style.

`Db::has_active_job` / `Db::dedupe_pending_jobs` exist specifically to keep
repeated Sync clicks (or a duplicate job already queued) from re-enqueueing
work — `run_match_job` also short-circuits immediately if a `matches` row
already exists for that URI, so even a duplicate job that's already queued
becomes a no-op instead of re-searching.

### The MP4/AAC decode bug (important, don't "fix" it back)

`rodio` 0.20.1's symphonia-backed MP4 decoder panics
(`unreachable!("Seek errors should not occur during initialization")`) on
essentially every AAC-in-MP4 (`.m4a`/`.mp4`/`.m4b`) file — confirmed against
raw yt-dlp output and a from-scratch ffmpeg remux alike, not a
malformed-file edge case. Consequences, all deliberate:

- `spotisync::fetch` never ships a `.m4a`: everything gets remuxed
  (`-c:a copy`, lossless) into raw ADTS `.aac` before being tagged and moved
  into the library. `PLAYABLE_EXTENSIONS` in `fetch.rs` does **not**
  include m4a/mp4/m4b for this reason.
- `Cargo.toml`'s release profile uses `panic = "unwind"`, not the `"abort"`
  PLAN.md §2 specifies, so that `engine::open_decoder`'s
  `std::panic::catch_unwind` around `Decoder::new` can actually catch a
  future decoder panic (this one or otherwise) instead of aborting the
  whole process. Don't switch this back to `"abort"` without also removing
  the crash-safety guarantee.
- `melodie --repair-audio` (`main.rs`) is the one-time migration for
  libraries with tracks downloaded before this fix: remuxes any surviving
  `.m4a`/`.mp4`/`.m4b` to `.aac` in place, restores tags from the DB (ADTS
  has no container-level metadata slot, so a remux alone produces a
  tag-less file — ask before assuming a "no tags" `.aac` means the audio is
  bad), and rewrites every playlist's `.m3u8`. Because on-disk extension
  isn't a reliable signal of "already fixed" vs "fixed but re-tag failed
  partway", its per-track dispatch is by current extension, not a
  done/not-done flag.

### Schema

`db.rs` owns the SQLite schema (`tracks`, `playlists`, `playlist_tracks`,
`spotify_tracks`, `matches`, `jobs`) — matches PLAN.md §4 exactly, and it's
the one place a schema change belongs (bump `SCHEMA_VERSION`, remember
`migrate()` destroys and recreates on mismatch, no data survives it by
design). Row structs (`Track`, `Playlist`, `MatchRow`) intentionally mirror
full DB rows rather than just whatever the current UI reads — they're
`#[allow(dead_code)]`-annotated for the fields nothing reads yet rather than
trimmed, so `get_track`/`list_tracks`/etc. stay trustworthy for the next
caller (a Subsonic server, a "loose tracks" view, whatever's next).

### UI

`fltk-rs`, retained-mode, hand-rolled virtualised list (`ui/list.rs`) — only
visible rows are laid out on `draw()`, not a scrollable container of real
widgets. Widgets are cheap to `.clone()` (they're handles to the same
underlying FLTK object), which is how the same button/list ends up captured
in multiple closures across `app.rs`. Closing the main window calls
`.iconize()`, not the default close behavior — the app is meant to keep
playing/serving MPRIS with no window open, and iconizing (vs. hiding) is
what keeps `fltk_app.wait()` from returning false as if there were no
windows left. `Ctrl+Q` or an MPRIS `Quit` are the real exit paths.

Buttons use `ui::theme::BUTTON_FRAME` (`RFlatBox`) plus
`ui/mod.rs::add_hover` for `Event::Enter`/`Event::Leave`-triggered color
swaps, and the track list tracks a `hover: Option<usize>` row updated on
`Event::Move`/`Event::Leave` (`ui/list.rs`). Both are purely event-driven —
no redraw timers, no per-frame polling — which is what keeps idle CPU at
effectively 0%. Any future hover/animation-ish polish should follow this
pattern, not a ticking redraw.

### Testing UI changes without disturbing a running instance

`Config` has no env-var override — it always resolves `config.toml`,
`melodie.db`, `state.toml`, and the library root off `$HOME`/XDG dirs
(`config.rs`). To visually test a UI change without touching the user's
real library/DB or racing a real instance's SQLite/`.m3u8` writes, run a
second copy with `HOME` pointed at a scratch directory (a couple of
tagged/untagged `.wav` files under `<scratch>/Music/Melodie/` scan fine —
`wav` is in `AUDIO_EXTENSIONS` and `lofty` falls back to filename/"Unknown
Artist" style tags gracefully).

Both windows end up titled exactly `"Melodie"` (`ui/mod.rs::build`), so a
screenshot tool matched by window title (e.g. `import -window Melodie`)
can silently grab the real, already-open window instead of the sandboxed
one — including whatever the user is actually playing/syncing. Disambiguate
by PID instead: `xprop -root _NET_CLIENT_LIST` lists window ids, `xprop -id
<id> _NET_WM_PID` gives the PID to match against the sandboxed process you
spawned, then screenshot that exact window id.
