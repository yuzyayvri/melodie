# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Melodie: a small, private, offline-first music player. One Rust binary crate.
Phases 0–3 of `PLAN.md` are implemented (library scan, playback, MPRIS/
background playback, `.m3u8` playlists, SpotiSync's Exportify-CSV sync path).
Phases 4–5 are also implemented: a read-only LAN/Subsonic server
(`src/server.rs`, `src/ui/pair.rs`, gated behind the default-on `lan` cargo
feature — see "LAN/Subsonic server and Android companion" below) and an
Android companion app under `android/` (`docs/android.md`, `docs/lan.md`).
Beyond PLAN.md's phases, both the desktop window and the Android app now
have search/sort/shuffle and (Android only) a top-bar back-navigation pass
— see "Search, sort, and shuffle (desktop)" and the Android nav/search/
sort/shuffle paragraphs under "LAN/Subsonic server and Android companion"
below. Phases 6–7 (live librespot sync, CI hardening) are not built. Read
`PLAN.md` before making architectural changes — it's the design doc this
was built from, cites its research, and documents the tradeoffs. Read
`README.md`'s "Deviations from PLAN.md" section too; several are
load-bearing bug workarounds, not style choices.

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

### Search, sort, and shuffle (desktop)

`app.rs`'s `ViewState` (`search: String`, `sort: SortField`, `shuffle:
bool`, held in the same `Rc<RefCell<_>>` pattern as `current_queue`) is the
single piece of state the search `Input`, sort `Choice`, and shuffle
`Button` in `ui/mod.rs` all write to. `apply_view(tracks, &view)` is the
pure function (filter by substring across title/artist/album, then sort or
Fisher-Yates shuffle) and `refresh_view(...)` is the only place its output
turns into both `list` (the UI) and `current_queue` (what Play/Next/
Previous actually load) — every caller that used to rebuild the list by
hand (`repopulate_from`, the playlist-choice callback, the new search/sort/
shuffle callbacks, `handle_worker_event`'s `SyncFinished`/`LibraryChanged`
paths) goes through it now. Don't reintroduce a second tracks→list
conversion; add a new caller of `refresh_view` instead.

Shuffle uses a hand-rolled `SystemTime`-seeded xorshift64 PRNG
(`shuffle_in_place`), not the `rand` crate — `rand`/`fastrand` aren't a
direct dependency (only transitive, via other crates) and one Fisher-Yates
pass doesn't justify adding one. This mirrors the Android side's opposite
tradeoff: mobile shuffles via Media3's built-in `shuffleModeEnabled`
because that dependency is already there, so hand-rolling would be the
wrong direction on that platform. Neither switching desktop to a real RNG
crate nor hand-rolling a reorder on Android is an improvement — each
platform already has the cheaper option for what it has installed.

None of this touches `engine.rs`: switching playlists, searching, sorting,
or toggling shuffle only ever stages `current_queue`, which the engine
doesn't see until a double-click, Next, or Previous asks it to load
something. Playback is never interrupted by typing in the search box.

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

### LAN/Subsonic server and Android companion

Everything here is gated behind the `lan` cargo feature, which is **on by
default** (`default = ["lan"]` in `Cargo.toml`) — "off by default" per
PLAN.md §7 means the *server* refuses to bind unless `config.toml` sets
`lan_enabled = true`, not that the feature is compiled out. If the Pair
button, `--pair`, or the LAN server all seem to be simply missing from a
build, check whether that build was actually produced with default
features before suspecting the UI code — `cargo build --release
--no-default-features` (or a stale binary built before this feature
existed) silently drops the button, `ui/pair.rs`, and `server.rs` with no
build warning, since `#[cfg(feature = "lan")]` compiles them out entirely
rather than disabling them at runtime.

`server.rs` is a hand-written OpenSubsonic subset (`tiny_http`, XML via
`format!`, deliberately no serialization crate for a dozen response
elements), spawned on its own thread by `server::spawn`. It never writes to
the library; every endpoint is read-only.

**Clicking Pair is itself the opt-in**, not just a QR display. PLAN.md
§7's "off by default" is a runtime privacy default (`config.toml`'s
`lan_enabled`), and the button that's supposed to turn pairing on used to
just `eprintln!` a "go edit config.toml and restart" message and return if
it was still off — invisible to anyone not watching a terminal, so the
button appeared to do nothing at all. `app.rs::run()` now keeps a single
`Rc<RefCell<(Config, Option<ServerHandle>)>>` (`lan_state`) shared between
the startup auto-start (if `lan_enabled` was already `true` on disk) and
the button's callback: whichever of the two runs first — startup or the
first click — is the one that calls `ensure_lan_token()`, `Config::save()`,
and `server::spawn()`, and it stores the resulting handle back in the
shared cell. The other path (or a second click) just sees `state.1 ==
Some(_)` and shows the QR immediately. Sharing one `Config`/handle instead
of each path taking its own `cfg.clone()` is what makes this
race-proof — there is structurally only one place either the token or the
running server can come from, not a convention to remember. Don't
reintroduce a second `cfg.clone()` + `ensure_lan_token()` call anywhere in
this path; add a new consumer of `lan_state` instead.

The Android app (`android/`) is a deliberately dependency-starved Subsonic
client: exactly three third-party deps
(`media3-exoplayer`, `media3-session`, `zxing-android-embedded`), enforced
by not pulling in `androidx.activity`/AppCompat — which is why
`PairActivity`'s QR scan uses the classic `startActivityForResult` API
instead of `ScanContract`/`registerForActivityResult` (see the doc comment
at the top of `PairActivity.kt`). `Ui.kt` builds every screen's views in
code for the same reason (no layout inflation, no XML resources); its
palette is meant to track `src/ui/theme.rs`'s hex values by hand, not via
any shared source of truth, so a desktop theme change needs a manual
matching edit there. `ScanOptions` must **not** get `.setOrientationLocked(false)`
— that was tried once and made the scanner activity rotate to landscape on
its own (full-sensor mode) instead of staying locked to the app's normal
portrait orientation; the default (locked) is correct and matches every
other screen in the app.

`LibraryActivity`'s offline fallback (`showOfflineFallback`) is the
mobile-side counterpart of "filesystem is the source of truth" above: if
the server is unreachable, it synthesizes playlists from whatever
`LocalLibrary` already has on disk (marked with an empty `RemotePlaylist.id`
so the rest of the screen knows to skip network calls for it) rather than
stranding already-synced tracks. Sync itself only ever runs in the
foreground on an explicit tap — no `WorkManager`, no background scheduler,
by design (`docs/android.md`'s "Deliberate limits").

Every non-root screen (`LibraryActivity`'s song list, `PlayerActivity`)
gets an on-screen back affordance via `Ui.topBar(context, title, onBack)`
— a hand-rolled back chevron + bold title row, not a `Toolbar`/`ActionBar`
(would need AppCompat). Root screens (`LibraryActivity`'s playlist list,
`PairActivity`) pass `onBack = null`. `Ui.column()`, the root view every
screen builds on, also pads itself for the status bar/camera cutout/nav bar
via a `setOnApplyWindowInsetsListener` using the deprecated-but-functional
`systemWindowInsetTop`/`systemWindowInsetBottom` fields — chosen over the
API-30 `WindowInsets.Type` family specifically so there's no `SDK_INT`
branch needed against `minSdk = 24`. Fixed once in `column()`, not
per-activity, since every screen routes through it.

`LibraryActivity.kt`'s `filterAndSortSongs(songs, query, sort)` is a pure,
framework-free top-level function (JVM-testable, `LibraryActivityTest.kt`)
mirroring desktop's `apply_view` above — same filter-then-sort shape, same
case-insensitive substring match. Its consumer, `renderSongs(playlist)`,
is the single place the song list's rows, status text, sync-button
visibility, and tap-to-play queue get built — **the queue is built from
the displayed (filtered/sorted) list, not the server's raw `songs` order**;
an earlier version queued from `songs` regardless of what was on screen,
which would have silently diverged the moment sort landed. Don't
reintroduce a second queue-building path that reads `songs` directly.
`renderSongs` itself only handles rendering — the local-library lookup and
sync-button visibility are computed once per playlist-open/sync-complete
(`showSongs`/`startSync`'s `onDone`) and cached in `localForPlaylist`/
`haveIdsForPlaylist` fields, not recomputed from disk on every keystroke.

Shuffle (`PlayerActivity`) toggles Media3's native
`controller.shuffleModeEnabled` directly rather than reordering the queue
by hand — see the desktop comparison above. State survives across songs via
`Queue.shuffleEnabled`, applied to the controller in `connect()` right
after `controller = c` (and thus after the existing stale-controller-future
guard, not inside it). **`updateShuffleColor()` must toggle the button's
*text* color (`Ui.ACCENT`/`Ui.FG_DIM`), not its fill** — `Ui.button()`
hardcodes white-on-`ACCENT` text, so swapping the fill alone to `BG_ALT`
for the off state leaves near-invisible dark-on-dark text. This shipped
broken once already and was only caught by a final whole-branch review,
not any single task's review or the compiler — if you touch this button,
check it renders with both fill *and* text visible in the off state, not
just that it compiles.

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
