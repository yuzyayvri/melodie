# LAN/Subsonic server + Android companion — design

Status: approved. Covers PLAN.md Phases 4-5.

Two independent sub-projects, built in sequence: Phase 4 (server) stands
alone and is testable with `curl`; Phase 5 (Android) depends on Phase 4's
endpoints.

---

## Phase 4 — Subsonic server (`src/server.rs`, feature `lan`)

### Dependencies (new)

- `tiny_http` — sync HTTP, no async runtime (named in PLAN.md §2).
- `fastrand` — token generation. Tiny, no-dep alternative to `rand`.
- `qrcode` — encodes the pairing payload into a bit matrix; Melodie draws it
  itself (no PNG/image-encoding crate needed — a hand-written PPM or direct
  `fltk::image::RgbImage` buffer from the matrix is a few lines).
- `md5` — Subsonic's salted-token auth scheme (`t = md5(token + salt)`).

All four are small, sync, dependency-light — consistent with PLAN.md §2's
"no async runtime, no GPU stack" rule.

### Cargo feature

New `lan` feature gates `server.rs`, the pairing button in `ui/mod.rs`, and
the four deps above. Not enabled by default. Matches the `features:
live-spotify, lan, tray` skeleton in PLAN.md §4 (only `lan` gets real code
this pass — README already explains why empty stub features weren't added
for the others).

### Config additions (`config.rs`)

```rust
pub lan_enabled: bool,       // default false
pub lan_port: u16,           // default 4533 (Subsonic convention)
pub lan_bind: String,        // default "0.0.0.0"
pub lan_token: String,       // auto-generated via fastrand on first enable
pub lan_allow_public: bool,  // default false
```

Startup refuses to bind if `lan_bind` parses to a non-private/non-loopback
address and `lan_allow_public` is false — PLAN.md §9's LAN-exposure risk
mitigation.

### Endpoints

Read-only OpenSubsonic subset, XML responses only (no `f=json`, no
quick-xml dependency — hand-written `format!` plus one escaping helper is
enough for output this simple):

- `ping.view`
- `getLicense.view` — not in PLAN.md's list, but real clients (DSub,
  Ultrasonic, Symfonium, substreamer) refuse to proceed without a valid
  license response. Root-cause compatibility fix for the stated goal ("every
  existing Subsonic client works"), not scope creep.
- **Folder family**: `getIndexes.view`, `getMusicDirectory.view`
- **ID3 family**: `getArtists.view`, `getArtist.view`, `getAlbum.view`
- `getPlaylists.view`, `getPlaylist.view`
- `stream.view` — supports a single `Range:` header for seeking.
- `getCoverArt.view` — resolves an id to a representative track, extracts
  the embedded picture via `lofty` on demand. No new `covers/` bookkeeping;
  the existing `covers/` dir (keyed by YouTube video id, spotisync-only) is
  left untouched, this is a separate on-demand read path.
- `scrobble.view` — no-op, returns success.

Both artist/album families are implemented because different real clients
hard-require one or the other; they share one grouping function computed
over `Db::list_tracks()` (grouped by `artist`, then `(artist, album)`), so
giving both costs little.

### ID scheme

No new schema, no persisted artist/album tables. An artist "folder" id is a
stable hash of the artist string; an album id is a stable hash of
`(artist, album)`; a track id is `"t:<tracks.id>"` (maps straight to the
existing integer primary key). Computed fresh per request — matches "DB is
a rebuildable cache" and needs no migration.

### Threading

`server::spawn(cfg, db) -> JoinHandle` follows the existing
`spotisync::spawn` pattern (`Arc<Db>`, cloned `Config`). Two fixed worker
threads each call `tiny_http::Server::recv()` in a loop (the crate's
documented multi-threaded usage pattern) — enough that a slow `stream`
request doesn't block a concurrent `getCoverArt` call, without a thread
pool. `ponytail: fixed at 2 threads, revisit if more than one phone client
becomes real.`

### Pairing

A feature-gated "Pair Phone" button in the desktop UI opens a small window
showing the QR code (encoding `melodie://<lan_bind resolved to a real LAN
IP>:<port>/<token>`) plus the same info as text, for the manual-entry
fallback on the Android side. No mDNS (see below).

### Explicitly skipped

- **mDNS advertisement** (PLAN.md §8 mentions it) — QR pairing already
  hands the phone host+port+token directly, so discovery-without-scanning
  buys little for a single-user pairing flow, at the cost of a dependency
  and idle background chatter that conflicts with §6's CPU budget. Decided
  with the user during design.
- **JSON responses** (`f=json`) — XML is the Subsonic default and every
  client supports it; skips a serialization-format branch and a dependency.

### Tests

- Pure-logic unit tests: XML escaping, id-hash determinism, salted-auth
  check (`md5(token + salt)`), `Range:` header parsing.
- One integration test boots the server on an ephemeral port and talks to
  it over a raw `std::net::TcpStream` (hand-written minimal HTTP GET) — no
  new HTTP client dependency needed just for tests.

---

## Phase 5 — Android companion (`android/`)

New Gradle project, Kotlin, Views (no Compose), `minSdk 24`. Desktop is the
source of truth; the phone only mirrors what it's told to.

### Dependencies

- `androidx.media3:media3-exoplayer` + `media3-session` — non-negotiable
  per PLAN.md §1/§7 for background playback via `MediaSessionService`.
- `com.journeyapps:zxing-android-embedded` — QR scanning. Chosen over ML
  Kit Barcode Scanning specifically because ML Kit requires Google Play
  Services and pulls in a much larger dependency tree; ZXing-embedded works
  on stock AOSP, is a few hundred KB, and delegates to its own scanning
  Activity so the app doesn't hand-roll camera permission/preview code.

Everything else is Android/Kotlin stdlib — no Room, no OkHttp, no
WorkManager, no coroutines:

- `HttpURLConnection` for the Subsonic HTTP calls.
- `XmlPullParser` (`Xml.newPullParser()`) to read the XML responses —
  matches the server's XML-only output, needs no parsing library.
- `org.json` for a local `library.json` manifest (list of synced
  playlists/tracks + local file paths).
- Plain `Thread` + `Handler(mainLooper)` for the sync pass — one track
  downloaded at a time, mirroring the desktop's own serialized-download
  policy (PLAN.md §5.5), posting progress back to the UI thread. No
  scheduled/background sync; sync runs only while the app is open and the
  user taps Sync, which is all Tier 1 asks for.

### Screens (matches PLAN.md §7's "one service, one list screen, one
player screen, one sync worker")

1. **Pairing** (shown when no host/token stored in `SharedPreferences`) —
   "Scan QR" (ZXing `IntentIntegrator`) plus manual host/port/token entry
   fields as a fallback.
2. **Playlists → tracks list** — fetches `getPlaylists` from the server,
   tapping a playlist syncs it (selective, not the whole library — keeps
   phone storage bounded; this is a scope decision beyond PLAN.md's literal
   text but necessary for it to be usable on a real phone). Synced tracks
   download into app-private storage (`filesDir/melodie/tracks/`) and
   `library.json` is updated.
3. **Player** — `MediaSessionService` + `ExoPlayer` playing from the local
   file; lockscreen/notification controls come from Media3 automatically.
   Tapping a track in the list, or the notification, opens this.

### Storage layout (phone)

```
<filesDir>/melodie/tracks/<track_id>.<ext>
<filesDir>/melodie/library.json          # synced playlists/tracks manifest
SharedPreferences "pairing": host, port, token
```

### Build & packaging

No Android SDK/Gradle/JDK-Kotlin toolchain present on this machine besides
JDK 21. Plan: bootstrap Android cmdline-tools + `platforms;android-34` +
matching build-tools under `~/Android/Sdk` (additive, doesn't touch
anything existing), generate the Gradle wrapper, build a **debug-signed**
APK (Gradle's built-in debug keystore — no keystore for the user to manage,
per the user's explicit choice). Apply `minifyEnabled`/`shrinkResources` on
the debug build type to pull size down from Media3's baseline. Report the
actual measured APK size rather than assume PLAN.md §7's 2-4MB figure,
which was written before accounting for Media3's real footprint.

Deliverable: the `.apk` file plus sideload instructions (transfer via
USB/cloud/email, tap it, allow "install unknown apps" once).

### Explicitly out of scope this pass

- Background/scheduled sync (WorkManager) — button-triggered sync only.
- Whole-library mirroring — playlist-selective sync only.
- Any matching/downloading/Spotify logic on the phone — PLAN.md §7 is
  explicit that the phone must never run this.

---

## Verification plan

- `cargo test` covers the new unit + integration tests for `server.rs`.
- Manual `curl` smoke test against a running `--features lan` build
  (`ping.view`, `getIndexes.view`, `stream.view` with a `Range:` header).
- `./gradlew assembleDebug` produces the APK; report its measured size.
- No Android emulator available — the built APK is handed to the user for
  on-device verification, per the task's stated workflow.
