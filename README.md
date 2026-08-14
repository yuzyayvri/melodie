# Melodie

A small, private music player. See `PLAN.md` for the full technical plan.
This build implements **Phases 0–5**: library scan, playback, MPRIS/tray-less
background playback, `.m3u8` playlists, SpotiSync's Exportify-CSV path, a
read-only LAN/Subsonic server with QR pairing, and an Android companion app.
Phases 6–7 (live librespot sync, CI hardening) are not implemented.

## Building

Requires a stable Rust toolchain, a C/C++ toolchain, `cmake`, and `git` (fltk-rs
builds FLTK from source). On Linux you'll also need the usual X11/Xft/Xrender/
Xinerama/fontconfig/pango dev headers (already present on most desktops) and
either ALSA or PulseAudio for `cpal`.

```sh
cargo build --release
```

The binary is `target/release/melodie`. Debug builds (`cargo build`) work the
same way and compile faster.

## Running

```sh
melodie              # opens the player window
melodie --rescan     # scans the library and prints it, no window
melodie --pair       # prints this machine's LAN host/port/token, no window
melodie --help
```

On first run, `melodie` writes a config file and creates:

```
~/.config/melodie/config.toml
~/.local/share/melodie/melodie.db       # cache; safe to delete
~/.local/share/melodie/covers/
~/.local/share/melodie/state.toml       # last volume/track/position/loop mode
~/Music/Melodie/                        # library root (edit config.toml to change)
~/Music/Melodie/_playlists/             # .m3u8 files, read and written here
~/Music/Melodie/_inbox/                 # drop Exportify CSVs here
```

Drop audio files (mp3/flac/m4a/mp4/aac/ogg/oga/wav) anywhere under the
library root and they'll show up after a scan (the app rescans on launch,
or run `melodie --rescan`). Drop a `.m3u8` under `_playlists/` and it shows
up in the playlist dropdown. Drop an [Exportify](https://exportify.net) CSV
under `_inbox/` and click **Sync** — SpotiSync will match tracks against
YouTube via `yt-dlp` and download the ones it's confident about; anything
scored 40–75 lands in the **Review** window instead of downloading
automatically.

`yt-dlp` must be on `PATH` for SpotiSync to do anything; playback and the
rest of the app work fine without it. `ffmpeg` is optional — it's only used
as a transcode fallback if a download comes back in a format Symphonia can't
decode (see PLAN.md §1, the Opus gap).

### Keyboard controls

| Key | Action |
|---|---|
| Space | Play/Pause |
| Left / Right | Seek -5s / +5s |
| Ctrl+Left / Ctrl+Right | Previous / Next track |
| Ctrl+Q | Quit |

Closing the window iconizes it instead of quitting — playback and MPRIS
controls keep working. Media keys, `playerctl`, and any other MPRIS client
control playback (`org.mpris.MediaPlayer2.melodie`); `Raise` restores the
window, `Quit` exits for real.

## What's implemented

- **Phase 0** — config, SQLite schema + migrations (drop-and-recreate on
  version mismatch, since the DB is a rebuildable cache), incremental
  library scan (mtime+size) with `lofty` tag reads, `--rescan` CLI.
- **Phase 1** — `rodio`+`symphonia` playback engine on its own thread
  (gapless via a 2-track look-ahead buffer, seek, volume), FLTK window,
  virtualised track list (only visible rows are drawn/laid out), keyboard
  controls, `state.toml` persistence (volume, last track, position).
  Also (beyond PLAN.md): loop modes (off/repeat-queue/repeat-track,
  persisted), and a UI pass — rounded buttons, event-driven hover feedback,
  a playing-row accent bar — with no added idle CPU/RAM cost.
- **Phase 2** — `souvlaki` MPRIS integration (play/pause/next/prev/seek/
  volume/raise/quit), window-close backgrounds instead of quitting,
  playlists read from and written to `.m3u8`, synced into the DB cache.
- **Phase 3** — Exportify CSV parser, a worker thread draining a `jobs`
  queue (`match` then `download`), the scoring table from PLAN.md §5.4,
  a serialized `yt-dlp` fetcher that tags+embeds art via `lofty` and moves
  the file into the library, and a Review window for the 40–75 score band.
- **Phase 4** — a read-only OpenSubsonic-compatible LAN server (`tiny_http`,
  two fixed worker threads, no async runtime), token auth, browse/playlist/
  stream/cover-art endpoints over the existing DB cache, and phone pairing:
  a **Pair** button (and headless `melodie --pair`) that shows the host,
  port and token as both text and a QR code. mDNS advertisement was
  deliberately skipped — see Deviations below.
- **Phase 5** — an Android companion app (Kotlin, Views, `android/`): QR
  pairing against the Phase 4 LAN server, playlist-selective offline sync
  into app-private storage, and background playback via a Media3
  `MediaSessionService` (lockscreen/Bluetooth controls, works in airplane
  mode once synced). Shrunk release build measures **~1.1 MB**; see
  `docs/android.md`.

Verified against a real `yt-dlp`/`ffmpeg`/audio-device environment during
development: CSV → search → score → auto-accept → download → tag → library
→ playlist `.m3u8` all confirmed working end to end, and MPRIS confirmed
working against `playerctl`/`dbus-send`.

## Android companion

A phone client lives under `android/` and pairs with the LAN server above to
browse playlists, sync tracks to app-private storage, and play them back in
the background with lockscreen controls. The built APK is at
`melodie-companion.apk` in the repo root — see `docs/android.md` for
installing, pairing, and build instructions.

## Deviations from PLAN.md

- **souvlaki's Linux backend uses `dbus-rs` (libdbus), not the "pure-Rust
  zbus backend"** the plan's research section describes. souvlaki 0.7
  defaults to its `use_dbus` feature; `use_zbus` exists but isn't the
  default. Functionally equivalent, just not pure-Rust for that one piece.
- **Downloaded files are named `<Title>.<ext>`, not `<NN> <Title>.<ext>`.**
  Exportify CSVs don't carry a reliable per-album track number, and using
  playlist position would risk downloading the same track twice if it
  appears in two playlists (matches/downloads are global, keyed by Spotify
  URI, per PLAN.md §5.3). If the exact same (artist, title, album) is
  downloaded from two different Spotify URIs, the second reuses the first
  file's path rather than being kept side by side — the ceiling PLAN.md's
  own `ponytail:` guidance for this kind of simplification: rare in a
  personal library, revisit with a disambiguating suffix if it bites.
- **Cover art is not resized to a 128px thumbnail.** The original embedded/
  downloaded image is stored as-is in `covers/` and embedded in the file's
  tag; FLTK's image widgets scale at draw time. Skips a dependency
  (`image`) for something the UI doesn't render yet — add real resizing
  when covers actually show up somewhere.
- **Review screen shows one candidate per track, not the top three.** The
  `matches` table (PLAN.md §4 schema, used as given) has one
  `youtube_id`/`score` slot per Spotify URI, so there's nowhere to persist
  three ranked candidates without changing the schema. The single stored
  candidate's title/channel/duration is logged to stderr when a track is
  matched, and its score and `youtu.be/<id>` are shown in the Review list
  so a human can still sanity-check it before confirming.
- **No automatic retry for low/no-score matches.** PLAN.md §5.4 says a
  `< 40` match is "retried on the next sync only if the query changes" —
  tracking query fingerprints wasn't worth it for this pass. Any existing
  `matches` row (confirmed or not) is treated as final; re-matching would
  currently mean deleting that row by hand.
- **No "Loose tracks" cleanup view.** PLAN.md §5.3 mentions listing tracks
  in no playlist for manual cleanup; not built this pass (not one of the
  phase 0–3 "done when" criteria). The `orphan_tracks`-style query is
  trivial to add back — `SELECT * FROM tracks WHERE id NOT IN (SELECT
  track_id FROM playlist_tracks)`.
- **`Cargo.toml` doesn't declare the `live-spotify`/`lan`/`tray` features**
  named in PLAN.md §4's skeleton. There's no code behind them yet (phases
  4/6/2-optional); declaring empty features felt like scaffolding for a
  later pass rather than something this one should ship.
- **`docs/spotisync.md` and `docs/resource-budget.md`** (PLAN.md §4) are not
  populated — the resource-budget doc in particular depends on the
  `make bench` tooling from Phase 7, which is out of scope here.
- **Startup does a full incremental rescan + playlist sync on the UI
  thread**, not just on `--rescan`. It's cheap (mtime+size stat calls) for
  a personal library, but on a very large library it could push cold start
  past the ≤150ms target in PLAN.md §6. Worth moving to a background thread
  if that turns out to matter.
- **mDNS advertisement (PLAN.md §8) is not implemented** — QR pairing
  already carries host, port and token, so LAN discovery buys nothing for a
  single-user pairing flow and costs a dependency plus periodic background
  chatter against the §6 CPU budget.
- **`getLicense` and `getMusicFolders` are served although PLAN.md §7's
  list omits them**: real clients refuse to proceed without them.
- **Playlist-selective sync rather than whole-library mirroring.**
  PLAN.md §7 says the app "syncs files over the Tier 0 endpoint into
  app-private storage"; syncing everything would fill a phone, so sync is
  per-playlist and opt-in.
- **No background/scheduled sync.** Sync runs on demand while the app is
  open; WorkManager was not worth a dependency for a manual mirror.
- **The local sync manifest is a hand-rolled TSV file (`library.tsv`), not
  the `library.json` the original design spec mentioned.** `org.json`
  (Android's built-in JSON class) is a stub outside a real device, so it
  can't be exercised by a JVM unit test; a TSV manifest with its own tested
  `encodeLine`/`decodeLine` functions can be.
- **The Android APK is signed with the debug key**, deliberately, so there
  is no keystore to manage for a personal sideload.
- **Measured APK size vs PLAN.md §7's 2–4 MB estimate**: the shrunk release
  build is ~1.1 MB (1,161,133 bytes), under the low end of the estimate.

## Resource budget

Full measurement (RSS, CPU, cold start) is Phase 7's `make bench` tooling
and isn't built yet. `cargo build --release` does use the `lto = "fat"`,
`codegen-units = 1`, `panic = "abort"`, `strip = true` profile from
PLAN.md §2, which is most of the win. One number gathered by hand on this
build: **stripped binary is ~7.6 MB** (was ~6.9 MB pre-Phase-4; `tiny_http`,
`md5`, `fastrand` and `qrcode` for the LAN server cost ~0.7 MB) — over the
≤6 MB target in PLAN.md §6 but under the 10 MB hard ceiling. `rusqlite`
(bundled SQLite), FLTK, the Symphonia decoders, and `souvlaki`'s `dbus`
backend are the main contributors; trimming would mean profiling which of
those actually matters, which is exactly what Phase 7 is for.

## Tests

```sh
cargo test
```

Covers the DB layer (upsert/backoff semantics), the incremental-scan-safe
engine gapless/queue-advance logic (against a real audio device — skips
cleanly if none is available), state persistence, Exportify CSV parsing,
the matcher's scoring function, and playlist `.m3u8` round-tripping.
