# Melodie — Technical Plan

A small, private music player. Named by the user's girlfriend. The whole point is that it stays out of the way: it starts instantly, it plays music, it keeps a local library in sync with Spotify playlists, and it never asks anyone to log in.

Status: planning document.

---

## 1. Research summary

Findings that actually changed a decision. Each one is cited.

**GUI toolkits are the single largest RAM risk, and immediate-mode is not automatically the lean choice.**
A reported measurement of a simple egui/eframe app with a handful of buttons and labels puts it at ~30 MB RSS with symbols stripped, while an equivalent fltk-rs app sits at ~1 MB; egui binaries land around 3–4 MB stripped ([egui issue #3689](https://github.com/emilk/egui/issues/3689)). egui also redraws whole frames, and a production user notes that constant repainting means constant CPU activity and measurable laptop power drain ([Tritium: Rust GUI observations](https://tritium.legal/blog/desktop)). That single data point eats the entire 30–40 MB budget before a single track is loaded, so egui is out as the default. fltk-rs statically links FLTK into the executable, produces ~1 MB stripped binaries, needs no GPU, and explicitly targets old architectures ([fltk-rs README](https://github.com/fltk-rs/fltk-rs), [lib.rs/crates/fltk](https://lib.rs/crates/fltk)).

**Slint is the correct escape hatch, not the default.**
Slint is retained-mode with selective redraws, and its `SoftwareRenderer` is pure CPU with no GPU or graphics API required, targeting runtimes under 300 KiB of RAM on embedded devices ([Slint software renderer](https://deepwiki.com/slint-ui/slint/3.1-software-renderer), [Slint embedded](https://slint.dev/embedded)). It is a genuine option if FLTK's look becomes unacceptable, at the cost of a DSL, a build step, and a heavier dependency tree. Keep it as the documented fallback behind the UI boundary.

**Audio decoding: pure Rust is viable, with one hole — Opus.**
rodio sits on cpal for output and Symphonia for decoding, supports gapless playback via a builder flag and a queued next track ([rodio README](https://github.com/RustAudio/rodio)). Symphonia covers AAC, ALAC, FLAC, MP3, MP4, OGG, Vorbis, WAV, WebM — but Opus is still "in work"/unimplemented ([Symphonia docs](https://docs.rs/symphonia/)). YouTube's best audio-only stream is usually Opus. This directly dictates the download format policy (§5.5): fetch itag 140 / m4a AAC, not Opus, so playback stays pure Rust with no C decoder and no ffmpeg at play time.

**yt-dlp is still the right fetcher, and 2026 YouTube requires deliberate handling.**
YouTube binds PO Tokens to individual video IDs, so manual extraction is no longer viable; the guide's recommendation is a PO Token Provider plugin (`bgutil-ytdlp-pot-provider`, maintained by a yt-dlp maintainer, or `yt-dlp-getpot-wpc` as fallback), and `web_music` specifically requires a PO Token for stream requests ([yt-dlp PO Token Guide](https://github.com/yt-dlp/yt-dlp/wiki/PO-Token-Guide)). SABR-driven blocking got noticeably more aggressive through 2025–2026 ([yt-dlp issue #14390](https://github.com/yt-dlp/yt-dlp/issues/14390)). Consequence: downloading is treated as a fallible background job with retries and a visible failure state, never as something the player blocks on. The pure-Rust alternative, `rustypipe-downloader`, still needs ffmpeg plus a separate `rustypipe-botguard` binary ([rustypipe-downloader docs](https://docs.rs/rustypipe-downloader/latest/rustypipe_downloader/)) — it trades away the one thing yt-dlp is best at, which is shipping a fix within days of YouTube changing something. yt-dlp stays; rustypipe is noted as a contingency.

**ISRC lookup on YouTube Music does not work, so matching must be heuristic.**
Searching YouTube Music by ISRC via yt-dlp returns no results even though the browser works ([yt-dlp issue #15389](https://github.com/yt-dlp/yt-dlp/issues/15389)). The matcher therefore scores on artist/title tokens, duration delta, and "Topic"/Art Track channel signals, with a human review queue for low-confidence hits.

**Spotify's official API is no longer a dependable foundation for a personal app.**
Since 15 May 2025 extended quota applications are accepted only from registered organizations with a launched service and 250k+ MAU ([Spotify: updating the criteria for extended access](https://developer.spotify.com/blog/2025-04-15-updating-the-criteria-for-web-api-extended-access)). Development Mode was loosened again in July 2026 — up to 25 client IDs per developer account, with quota pooled per account ([Spotify: Web API quota updates](https://developer.spotify.com/blog/2026-07-23-web-api-quota-updates)) — which is fine for one user, but third-party reporting also describes tightened dev-mode conditions in early 2026 including a Premium requirement for the app owner ([Vorp Labs summary](https://vorplabs.com/agent-tools/spotify-cli)); that specific claim is not confirmed by Spotify's own blog. Conclusion: never make the API the only path. Exportify CSV — which exports Track URI, Track Name, Artist Name(s), Album Name, Added At, Duration, ISRC ([Exportify](https://github.com/watsonbox/exportify)) — is the guaranteed-to-work primary path.

**Reading a running Spotify client is possible but Premium-gated and fragile.**
librespot 0.8.0 (Nov 2025) ships `librespot-oauth` (auth code + PKCE, loopback listener) and `librespot-metadata` with playlist support ([docs.rs/librespot](https://docs.rs/librespot), [librespot-oauth](https://crates.io/crates/librespot-oauth)), which gets playlists without registering a developer app at all. But librespot only works with Spotify Premium ([librespot docs](https://docs.rs/librespot)). The desktop client's own `local-files.bnk` / `.bnk` cache files are undocumented binary blobs with no public parser — not a foundation to build on. So the "read the local client" requirement is satisfied two ways: librespot as a local Spotify client for playlists, and MPRIS (`org.mpris.MediaPlayer2.spotify`) to passively capture whatever the desktop app is playing right now.

**Navidrome is the right shape to copy, and the Subsonic API is free mobile support.**
Navidrome is a single static Go binary with SQLite as its only database dependency, idles around ~50 MB, and runs on a Pi Zero ([AppSelfHost repo insight](https://appselfhost.com/repo-insight-navidrome-webassembly-plugins-sqlite-storage-and-subsonic-api-in-one-lean-go-binary/), [DEV comparison](https://dev.to/selfhostingsh/navidrome-vs-emby-which-music-server-should-you-self-host-4ppj)). Its real leverage is implementing the Subsonic API, which unlocks a whole ecosystem of existing mobile clients ([OpenSubsonic](https://opensubsonic.netlify.app/docs/), [Navidrome Subsonic compatibility](https://www.navidrome.org/docs/developers/subsonic-api/)). A minimal read-only subset (`ping`, `getIndexes`, `getMusicDirectory`, `getPlaylists`, `stream`, `getCoverArt`) is a few hundred lines — see `smolsonic` for a Rust precedent ([smolsonic](https://github.com/tsirysndr/smolsonic)) — and buys a working phone experience before the companion app exists.

**Android: the platform's own media stack is non-negotiable for background playback.**
Reliable background audio on Android means a `MediaSessionService` holding the player, which is what keeps playback alive when the app is not visible and lets external controllers drive it ([Media3 background playback](https://medium.com/@khorassani64/using-media3-with-kotlin-jetpack-compose-f1c033acd016)). Slint does support Android via `android-activity` for Rust ([Slint Android](https://docs.slint.dev/latest/docs/slint/guide/platforms/mobile/android/)), but a `NativeActivity` app is the wrong host for a long-lived foreground media service. Minimal, Views-based, notification-first Android players exist in the sub-100 KB range ([Tiny Music Player](https://github.com/martinmimigames/tiny-music-player)), which sets the realistic bar for a lean companion.

**System media integration is a solved dependency.**
`souvlaki` gives one abstraction over MPRIS (Linux, pure-Rust zbus backend), Windows SMTC, and macOS Now Playing ([souvlaki](https://docs.rs/souvlaki)). No reason to write D-Bus plumbing.

---

## 2. Language & toolchain

**Rust, stable, one binary crate.** Not negotiable much: it is the only mainstream option that gives a statically linked native binary, no GC pauses, no runtime, real audio libraries, and the same core compiled for a LAN server. Go would work but drags a GC and a heavier idle heap; C/C++ would cost more time than it saves.

| Concern | Choice | Why |
|---|---|---|
| UI | `fltk-rs` (FLTK statically linked) | ~1 MB RSS class, ~1 MB binary, no GPU, retained-mode so idle CPU is zero |
| Audio output + queue | `rodio` (wraps `cpal`) | Gapless queue is already there; no reason to hand-roll a mixer |
| Decoding | `symphonia` (mp3, aac/mp4, flac, vorbis, wav) | Pure Rust, no ffmpeg at play time |
| Metadata | `lofty` | Reads *and writes* tags, so downloaded files are self-describing |
| Database | `rusqlite` (`bundled`) | SQLite compiles in; one file; rebuildable cache, not source of truth |
| OS media keys | `souvlaki` | MPRIS/SMTC/NowPlaying in one API |
| LAN server | `tiny_http` | Synchronous, no tokio, no async runtime in the binary at all |
| CSV | `csv` + `serde` | Exportify parsing, nothing more |
| Spotify live path | `librespot-core/-oauth/-metadata` (optional feature) | Playlists without a developer app |
| Fetching | external `yt-dlp` process | Only component that must track YouTube weekly |

**Explicitly not used:** tokio/async-std (an async runtime buys nothing when there are four threads and one user), any web view, any GPU renderer, wgpu, serde_json for the hot path, an ORM, a plugin system.

**Build profile.** `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = true`, `opt-level = 3`. Note the deliberate choice of `3` over `"z"`: decode throughput on a weak CPU matters more than two megabytes on disk. Targets: `x86_64-unknown-linux-gnu` primary; `x86_64-unknown-linux-musl` for a fully static build (headless/LAN mode; note that a GUI build still dynamically links X11/Wayland — "single static binary" means everything Melodie ships is in the executable, not that it refuses to talk to the display server); `x86_64-pc-windows-msvc` if ever needed.

**Runtime dependencies.** Playback has none. Syncing needs `yt-dlp` on `PATH`; `ffmpeg` is optional and only used if a fallback download lands on a format Symphonia can't read.

---

## 3. High-level architecture

One process. Four threads. No IPC, no daemon, no service manager.

```
                    ┌──────────────────────────────────────────┐
                    │  main thread — FLTK event loop           │
                    │  blocks in Fl::wait(); redraw on event   │
                    └───────┬──────────────────────▲───────────┘
                            │ Command              │ Event
                            ▼                      │
        ┌───────────────────────────────────────────────────────┐
        │  Engine (owns playback state, queue, position)        │
        │  crossbeam channels in/out; no locks held across I/O  │
        └───┬──────────────────┬────────────────────┬───────────┘
            │                  │                    │
   ┌────────▼──────┐  ┌────────▼────────┐  ┌────────▼─────────┐
   │ audio thread  │  │ worker thread   │  │ lan thread (opt) │
   │ cpal callback │  │ scan / sync /   │  │ tiny_http,       │
   │ rodio+symph.  │  │ yt-dlp jobs     │  │ Subsonic subset  │
   └───────────────┘  └────────┬────────┘  └──────────────────┘
                               │
                     ┌─────────▼──────────┐     ┌───────────────┐
                     │ SQLite (rusqlite)  │◄────┤ filesystem    │
                     │ cache & job state  │     │ = source of   │
                     └────────────────────┘     │   truth       │
                                                └───────────────┘
   souvlaki (MPRIS/SMTC) attaches to Engine — media keys work with no window.
```

Rules that keep this small:

- **The filesystem is the source of truth.** Audio files carry their own tags (written by `lofty` at download time), playlists are written as `.m3u8` next to the library. Delete `melodie.db` and everything rebuilds from a scan. This also means any other player can read the library, which kills a whole class of lock-in and migration work.
- **The UI is a view, never an owner.** It holds no playback state. It sends `Command` and renders `Event`. That is the one abstraction boundary worth having — it is what makes swapping FLTK for Slint a contained change rather than a rewrite.
- **Playback never blocks on the network.** Sync is a job queue on a worker thread. If YouTube breaks, music still plays.
- **Background playback is the default, not a mode.** Audio lives on its own thread; closing the window stops rendering, not sound. MPRIS keeps controls available.

---

## 4. Project skeleton

```
melodie/
├── Cargo.toml                # one crate; features: live-spotify, lan, tray
├── PLAN.md
├── README.md
├── src/
│   ├── main.rs               # config load, thread wiring, CLI (--headless, --rescan)
│   ├── config.rs             # ~/.config/melodie/config.toml (paths, LAN token, quality)
│   ├── db.rs                 # schema + migrations + typed queries
│   ├── library.rs            # incremental scan (mtime+size), path policy, tag read
│   ├── engine.rs             # Command/Event, queue, gapless, seek, volume, state
│   ├── mediakeys.rs          # souvlaki wiring
│   ├── ui/
│   │   ├── mod.rs            # window, layout, keyboard map
│   │   ├── list.rs           # virtualised track/playlist list (draws visible rows only)
│   │   ├── review.rs         # SpotiSync match-confirmation screen
│   │   └── theme.rs          # colours, fonts, spacing
│   ├── spotisync/
│   │   ├── mod.rs            # sync state machine, diffing, job scheduling
│   │   ├── exportify.rs      # CSV parser  (path B)
│   │   ├── live.rs           # librespot session + playlist fetch (path A, feature)
│   │   ├── watcher.rs        # MPRIS listener for the running Spotify client (path A')
│   │   ├── matcher.rs        # candidate scoring
│   │   └── fetch.rs          # yt-dlp process wrapper, retries, archive
│   └── server.rs             # OpenSubsonic read-only subset (feature = "lan")
├── android/                  # separate Gradle project, Kotlin + Media3
└── docs/
    ├── spotisync.md
    └── resource-budget.md
```

Runtime layout:

```
~/.config/melodie/config.toml
~/.local/share/melodie/melodie.db          # cache; safe to delete
~/.local/share/melodie/covers/             # 128px thumbnails, content-addressed
~/Music/Melodie/<Artist>/<Album>/<NN> <Title>.m4a
~/Music/Melodie/_playlists/<Playlist>.m3u8
~/Music/Melodie/_inbox/                    # drop Exportify CSVs here
```

Schema (minimal, and that is the point):

```sql
tracks(id, path UNIQUE, title, artist, album, track_no, duration_ms, mtime, size, added_at)
playlists(id, name UNIQUE, source, source_ref, synced_at)
playlist_tracks(playlist_id, track_id, pos, PRIMARY KEY(playlist_id,pos))
spotify_tracks(uri PRIMARY KEY, title, artist, album, duration_ms, isrc, first_seen)
matches(spotify_uri PRIMARY KEY, youtube_id, score, confirmed INTEGER, decided_at)
jobs(id, kind, payload, state, attempts, last_error, next_attempt_at)
```

---

## 5. SpotiSync

Three inputs, one pipeline. Everything converges on a normalized `DesiredTrack { uri?, title, artist, album, duration_ms, isrc? }` and a `DesiredPlaylist { name, ordered tracks }`, so the rest of the system does not care where the metadata came from.

### 5.1 Path B — Exportify CSV (primary, always works)

The user exports at [exportify.net](https://exportify.net) (their own Spotify login, no developer app, no credentials handed to Melodie) and drops the CSVs into `~/Music/Melodie/_inbox/`. Melodie parses `Track URI`, `Track Name`, `Artist Name(s)`, `Album Name`, `Duration (ms)`, `ISRC`, `Added At`. Playlist name comes from the CSV filename unless a `Playlist Name` column is present. This path has zero moving parts, no auth, no rate limit, and cannot be broken by a Spotify policy change — which, given §1, is exactly why it is the primary.

### 5.2 Path A — live read from the local Spotify client (optional feature `live-spotify`)

Two complementary mechanisms:

1. **librespot session.** `librespot-oauth` runs the PKCE flow against a loopback listener (a browser opens once, token cached in `~/.local/share/melodie/spotify_token.json`), then `librespot-metadata` fetches the user's playlists and their track metadata. This is Melodie acting as a local Spotify client — no developer app registration, no Web API quota. Gated behind a feature flag because it pulls a sizeable dependency tree and requires Premium.
2. **MPRIS watcher (Linux).** Subscribe to `org.mpris.MediaPlayer2.spotify` property changes. Whatever the desktop client plays gets recorded into `spotify_tracks`, so a "Heard on Spotify" list accrues passively and can be one-click promoted to a synced playlist. Ten lines of zbus, and it is the most literal reading of "read from a running local Spotify client."

If librespot auth fails or the account is not Premium, Melodie degrades to the CSV path with an explicit message. It never silently does nothing.

### 5.3 Diffing and sync semantics

For each playlist: snapshot desired state → compare against `playlist_tracks` → produce adds and removes.

- **Adds** create a `jobs` row (`kind=match` then `kind=download`).
- **Removes** only detach the track from the playlist. Files are never deleted automatically; orphans (in no playlist, never played) are listed on a "Loose tracks" screen for manual cleanup. Irreversible deletion driven by a remote source is exactly the wrong default.
- Everything is idempotent and resumable: kill Melodie mid-sync, restart, it continues. Jobs carry `attempts` and `next_attempt_at` for exponential backoff.
- A track already matched with `confirmed = 1` is never re-searched. Ever. That is what makes repeat syncs nearly free.

### 5.4 Matching (`matcher.rs`)

Query construction: `"{artist} {title}"`, stripped of `(feat. …)`, `- Remastered 20XX`, and bracketed noise, then `ytsearch8:` via `yt-dlp -J --flat-playlist`. ISRC is stored but not used as a search key — verified not to work (§1).

Score per candidate:

| Signal | Weight |
|---|---|
| Duration within ±2 s of Spotify duration | +50 (linear falloff to 0 at ±12 s) |
| Title token overlap with `artist + title` | +30 |
| Channel is `… - Topic` or has "Art Track" markers | +15 |
| Uploader name matches artist | +10 |
| Title contains live / cover / remix / sped up / nightcore, and query does not | −40 |
| Duration under 60 s or over 15 min | −60 |

`score ≥ 75` → auto-accept, download, `confirmed = 0`. `40–75` → queued to the **Review** screen with the top 3 candidates, artwork, and durations; one keypress confirms and sets `confirmed = 1`. `< 40` → marked "no good match", retried on the next sync only if the query changes. A confirmed match is a permanent decision recorded in `matches`.

### 5.5 Fetching (`fetch.rs`)

One subprocess call per track, serialized (one download at a time by default — a personal library is not a race, and it keeps yt-dlp under YouTube's radar):

```
yt-dlp --no-playlist --no-progress --newline
       -f "140/bestaudio[ext=m4a]/bestaudio"
       --sleep-requests 1 --retries 3
       -o "<tmp>/%(id)s.%(ext)s"
       -- <video_id>
```

Format policy is driven by the Symphonia Opus gap (§1): prefer itag 140 (m4a/AAC ~128 kbps), which is present on essentially every music upload. If only Opus is available, transcode with ffmpeg when present; otherwise mark the job failed with a clear reason rather than downloading something unplayable.

Post-download: `lofty` writes title/artist/album/track number/date and embeds the cover art, the file moves to its final library path, a row lands in `tracks`, and the `.m3u8` is rewritten. Cover art is also downscaled once to a 128 px thumbnail in `covers/`.

Reliability handling, straight out of the PO-token research: Melodie checks `yt-dlp --version` at startup and warns if it is more than ~30 days old; the config exposes `ytdlp_extra_args` so the user can add `--extractor-args`, a PO-token provider plugin, or `--cookies-from-browser` without a Melodie release. Failures are surfaced per track, never as a modal, and never block playback.

### 5.6 Legal note

This builds a personal offline copy of music the user already listens to, on their own machine. It stays private, single-user, and undistributed. That framing is deliberate and should stay in the README.

---

## 6. Resource strategy

Budget, enforced not hoped:

| Metric | Target | Hard ceiling |
|---|---|---|
| Binary (stripped) | ≤ 6 MB | 10 MB |
| RSS, idle with window open, 5k tracks | ≤ 25 MB | 40 MB |
| RSS, playing | ≤ 35 MB | 50 MB |
| CPU, paused/idle | 0.0 % | 0.2 % |
| CPU, playing (AAC 128 k, 2010-era laptop) | ≤ 2 % | 5 % |
| Cold start to window | ≤ 150 ms | 400 ms |

Tactics, in order of how much they buy:

1. **No async runtime, no GPU stack, no web view.** This is most of the win and it is free — it is a decision not to add things.
2. **Event-driven UI.** FLTK blocks in `Fl::wait()`; zero frames rendered when nothing changes. Genuinely 0 % idle CPU, unlike an immediate-mode redraw loop.
3. **Virtualised lists.** Only visible rows are constructed and drawn. Library size stops mattering above the fold.
4. **Nothing loads whole.** Symphonia streams from disk into a small ring buffer (~200 ms). A 10-minute FLAC costs the same memory as a 2-minute MP3.
5. **Bounded artwork cache.** Thumbnails only (128 px, pre-scaled on disk), LRU-capped at 32 decoded images. Unbounded image caching is a documented way to blow up a Rust GUI's RAM ([egui #5439](https://github.com/emilk/egui/issues/5439)) — cap it from day one.
6. **SQLite tuned small.** `PRAGMA journal_mode=WAL`, `cache_size=-2000` (2 MB), `mmap_size=0`, prepared statements cached. Query the DB for the current view; do not hold the library in RAM.
7. **Pause means pause.** On pause, the cpal stream is paused so the audio device stops waking the CPU. Timers are stopped, not left ticking.
8. **Incremental scans.** mtime + size comparison; a rescan of an unchanged library touches no tags.
9. **Measure every release.** A `make bench` target runs the binary headless, plays a track, and records RSS/PSS (`/usr/bin/time -v`, `smem`) and binary size into `docs/resource-budget.md`. Regression past the hard ceiling fails the build. Numbers that are not measured become numbers that are not true.

Deliberate ceilings, to be marked in code with `ponytail:` comments: single-threaded downloads (fine for one user; parallelise only if a first sync is unbearably slow), full playlist `.m3u8` rewrite instead of incremental patching (a few hundred lines of text), linear scan scoring over candidates (n ≤ 8).

---

## 7. Android companion

Two tiers. The first costs almost nothing and might turn out to be enough.

**Tier 0 — Melodie speaks Subsonic (desktop side, feature `lan`).**
`tiny_http` on a configurable port, bound to the LAN, serving a read-only OpenSubsonic subset: `ping`, `getArtists`/`getIndexes`, `getMusicDirectory`/`getAlbum`, `getPlaylists`, `getPlaylist`, `stream`, `getCoverArt`, and no-op `scrobble`. That is a few hundred lines and it makes every existing Subsonic Android client work immediately — including offline-capable ones. Auth: no accounts, but Subsonic clients require credentials, so Melodie generates a fixed random token stored in `config.toml`. It binds to the LAN interface only and refuses to start if asked to listen on a public address without an explicit override. This ships before any Android code exists.

**Tier 1 — the companion app (`android/`).**
Kotlin, Views (no Compose), `minSdk 24`, target APK in the 2–4 MB range. Media3 `ExoPlayer` inside a `MediaSessionService` — this is the researched, non-negotiable way to get background playback, audio focus, Bluetooth/headset controls, and lockscreen art that actually survives the app being backgrounded. Offline-first: the app syncs files over the Tier 0 endpoint into app-private storage and plays from disk, so it works with the phone in airplane mode. Pairing is a QR code shown by the desktop containing host, port, and token — no login, no account, matching the rest of the project.

What the phone deliberately does *not* do: no yt-dlp, no matching, no Spotify contact. The desktop is the librarian; the phone is a mirror. That keeps the Android side to roughly one service, one list screen, one player screen, and a sync worker.

**Rejected:** Slint-on-Android sharing the Rust core. It would reuse code, but `NativeActivity` is the wrong host for a long-lived media service, and the sharable logic (matching, downloading) is precisely the logic the phone must not run.

---

## 8. Roadmap

Each phase ends with something usable. No phase exists only to enable the next one.

| Phase | Scope | Done when |
|---|---|---|
| **0 — Skeleton** | Cargo crate, config, SQLite schema + migrations, library scan with `lofty`, CLI `--rescan` | `melodie --rescan` indexes a folder and prints the library |
| **1 — It plays music** | `engine.rs` (queue, gapless, seek, volume), FLTK window, virtualised list, keyboard control, persisted state | Plays a library end to end; measured against §6 budget |
| **2 — Lives in the desktop** | souvlaki/MPRIS, background playback when the window closes, playlists as `.m3u8`, tray (optional feature) | Media keys work with no window; playback survives close |
| **3 — SpotiSync, CSV path** | Exportify parser, diffing, job queue, matcher, `yt-dlp` fetcher, Review screen | Drop a CSV in `_inbox`, get a synced local playlist; bad matches are reviewable |
| **4 — LAN** | OpenSubsonic subset, token auth, mDNS advertisement | An off-the-shelf Subsonic client on the phone plays the library |
| **5 — Android companion** | Kotlin/Media3 app, QR pairing, offline sync, background service | Music plays on the phone offline, screen off, from a synced library |
| **6 — SpotiSync, live path** | librespot OAuth + playlist fetch, MPRIS watcher for the running client | Playlists sync without exporting a CSV; watcher captures what Spotify plays |
| **7 — Hardening** | Resource-budget gate in CI, crash-safe job resume, yt-dlp staleness check, packaging (single binary + `.desktop`), README | Budget regressions fail the build; a fresh machine is set up in one step |

Phases 0–3 are the actual product. 4–7 are what make it pleasant.

---

## 9. Risks and mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| YouTube tightens SABR / PO-token enforcement and downloads break | High — it has been trending that way since 2024 | yt-dlp stays external and user-updatable; `ytdlp_extra_args` passthrough for PO-token provider plugins and cookies; downloads are retriable background jobs; **playback of existing files is never affected**; `rustypipe-downloader` documented as contingency |
| Wrong track downloaded (live version, sped-up edit, wrong artist) | High without mitigation | Duration + channel-type scoring, hard penalties for live/remix/sped-up, Review queue for the 40–75 band, permanent `confirmed` flag, one-key "replace this match" |
| librespot path breaks (protocol change, non-Premium account) | Medium | Feature-flagged and never the primary path; explicit degradation message; CSV path is unaffected |
| Spotify tightens Development Mode further | Medium | The design never depends on the Web API at all |
| FLTK looks dated enough to be unpleasant | Medium | Custom theme (colours, spacing, a good font) applied from day one; UI sits behind the Command/Event boundary so a Slint software-renderer swap is contained, not a rewrite |
| Opus-only source for some track | Low | Format policy prefers m4a; optional ffmpeg transcode; explicit failure rather than an unplayable file |
| RAM creep as features land | Medium | Measured budget with hard ceilings enforced in CI from phase 1, not retrofitted at the end |
| SQLite corruption / bad migration | Low | DB is a rebuildable cache — files and tags are the source of truth; a failed migration deletes and rescans |
| LAN server exposed beyond the home network | Low but serious | Binds to LAN interface only, token required, refuses public binds without an explicit flag, read-only endpoints, off by default |
| Scope creep (lyrics, visualisers, recommendations, sync conflict resolution) | High, honestly | Anything not on the roadmap waits until phases 0–3 are in daily use. The measure of success is that it is boring and it is used |

---

## Sources

- [egui RAM usage issue #3689](https://github.com/emilk/egui/issues/3689) · [egui image loader RAM #5439](https://github.com/emilk/egui/issues/5439) · [Tritium: Rust GUI observations](https://tritium.legal/blog/desktop)
- [fltk-rs](https://github.com/fltk-rs/fltk-rs) · [lib.rs/crates/fltk](https://lib.rs/crates/fltk)
- [Slint software renderer](https://deepwiki.com/slint-ui/slint/3.1-software-renderer) · [Slint embedded](https://slint.dev/embedded) · [Slint Android](https://docs.slint.dev/latest/docs/slint/guide/platforms/mobile/android/)
- [rodio](https://github.com/RustAudio/rodio) · [Symphonia](https://docs.rs/symphonia/) · [lofty](https://github.com/Serial-ATA/lofty-rs) · [souvlaki](https://docs.rs/souvlaki) · [tiny_http](https://github.com/tiny-http/tiny-http)
- [yt-dlp PO Token Guide](https://github.com/yt-dlp/yt-dlp/wiki/PO-Token-Guide) · [yt-dlp SABR issue #14390](https://github.com/yt-dlp/yt-dlp/issues/14390) · [YouTube Music ISRC search issue #15389](https://github.com/yt-dlp/yt-dlp/issues/15389) · [rustypipe-downloader](https://docs.rs/rustypipe-downloader/latest/rustypipe_downloader/)
- [Spotify: extended access criteria](https://developer.spotify.com/blog/2025-04-15-updating-the-criteria-for-web-api-extended-access) · [Spotify: Development Mode quota updates](https://developer.spotify.com/blog/2026-07-23-web-api-quota-updates) · [Vorp Labs: 2026 dev-mode limits](https://vorplabs.com/agent-tools/spotify-cli)
- [librespot](https://docs.rs/librespot) · [librespot-oauth](https://crates.io/crates/librespot-oauth) · [Exportify](https://github.com/watsonbox/exportify)
- [Navidrome architecture](https://appselfhost.com/repo-insight-navidrome-webassembly-plugins-sqlite-storage-and-subsonic-api-in-one-lean-go-binary/) · [Navidrome resource notes](https://dev.to/selfhostingsh/navidrome-vs-emby-which-music-server-should-you-self-host-4ppj) · [OpenSubsonic](https://opensubsonic.netlify.app/docs/) · [Navidrome Subsonic compatibility](https://www.navidrome.org/docs/developers/subsonic-api/) · [smolsonic](https://github.com/tsirysndr/smolsonic)
- [Media3 background playback](https://medium.com/@khorassani64/using-media3-with-kotlin-jetpack-compose-f1c033acd016) · [Tiny Music Player](https://github.com/martinmimigames/tiny-music-player)
