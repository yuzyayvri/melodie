# Android companion — nav rework, insets fix, search/sort/shuffle — design

Status: approved by user in chat 2026-08-15. Covers `android/` only; no
desktop, server, or Gradle-dependency changes.

Companion to the desktop search/sort/shuffle work landed the same session
(app.rs's `refresh_view`/`apply_view`, ui/mod.rs's search Input/sort
Choice/shuffle Button). This spec deliberately mirrors that shape
(filter → sort → render) so the two apps stay conceptually consistent even
though the UI toolkits are unrelated.

Trigger: user report — the top of the app clips under the front camera on
some phones, there's no in-app way back from the player screen (gesture nav
only), and no search/sort/shuffle on mobile at all. Constraint carried over
from the rest of `android/`: **zero new Gradle dependencies** — everything
below uses either a plain platform API or Media3, both already present.

---

## 1. Camera-clip fix (root cause, not per-screen patch)

**Root cause**: `compileSdk`/`targetSdk` 36 (`android/app/build.gradle.kts`)
enforces edge-to-edge by default — content draws behind the status bar and
any display cutout. `Ui.column()` (`Ui.kt`), the root view every one of the
three activities builds its screen on, has no top-inset padding, so the
first child (today: a title label) renders under the cutout.

**Fix**: one `setOnApplyWindowInsetsListener` added inside `Ui.column()`
itself, not per-activity. Reads `insets.systemWindowInsetTop` /
`systemWindowInsetBottom` (the deprecated-but-still-functional
`WindowInsets` fields, available since API 20 — chosen over the API-30
`WindowInsets.Type` family specifically so there's no `Build.VERSION.SDK_INT`
branch to maintain against `minSdk = 24`) and adds them to the column's
existing padding. Because every screen's root is `Ui.column()`, this is the
one place the fix belongs — PLAN.md-style root-cause reasoning, matches
`CLAUDE.md`'s "fix it once, where all callers route through."

No manifest or theme changes needed; edge-to-edge is already the platform
default at this `targetSdk`, this just stops fighting it.

## 2. Navigation: `Ui.topBar()`

New helper in `Ui.kt`:

```kotlin
fun topBar(context: Context, title: String, onBack: (() -> Unit)?): LinearLayout
```

Horizontal row, hand-rolled (no `Toolbar`/`ActionBar`, consistent with the
rest of `Ui.kt`'s "views in code" approach): a small borderless "‹" button
(only added when `onBack != null`, min 48dp touch target) + a bold title
`TextView`, left-aligned, ellipsized if long. Same palette as everything
else (`Ui.FG`/`Ui.BG_ALT`).

Wired in:

- **`LibraryActivity`, playlist list** (`showPlaylists`): title "Melodie",
  `onBack = null` — this is Library's root state, nothing to go back to
  in-screen.
- **`LibraryActivity`, song list** (`showSongs`): title = playlist name,
  `onBack` = same transition `onBackPressed` already does
  (`showPlaylists()`). `onBackPressed` stays as-is — this adds the missing
  on-screen affordance, it doesn't replace gesture/system back.
- **`PlayerActivity`**: title "Now Playing", `onBack = { finish() }` — this
  is the concrete fix for "I have to rely on my phone's gesture navigation
  to go back after clicking a song."
- **`PairActivity`**: title "Pair with Melodie", `onBack = null` — cosmetic
  consistency only (it's the launcher/root screen already).

## 3. Search/sort — `LibraryActivity.showSongs` only

Two new views added below the top bar, both native platform widgets:

- `EditText` (via the existing `Ui.input()` style), placeholder "Search
  songs", `addTextChangedListener` filters live.
- `Spinner` (plain `android.widget.Spinner`, not an AppCompat spinner) with
  four items: Title / Artist / Album / Duration.

New activity-scoped state on `LibraryActivity`: `searchQuery: String`,
`sortField` (a 4-value enum mirroring `app.rs`'s `SortField`). A new
`renderSongs()` function replaces the current inline body of `showSongs`'s
list-building: filters `songs` by substring (title/artist/album,
case-insensitive) against `searchQuery`, sorts by `sortField`
(String fields case-insensitive; Duration numeric), then builds the
`ListView` adapter and the tap-to-play ordering from that same filtered/
sorted list — both text change and spinner-selection callbacks just call
`renderSongs()` again, same one-function-owns-the-view-state shape as
desktop's `refresh_view`.

**Bundled correctness fix**: today, `showSongs`'s click handler builds the
playback queue (`ordered`) from `songs` — the server's raw order — even
when the visible list has been re-sorted (there's no filter/sort yet, so
this hasn't been reachable, but it would silently diverge from what's on
screen the moment sort landed without this). `renderSongs()` makes the
queue built from the *displayed* order the only path, so screen and queue
can't disagree.

Search/sort do not touch the playlist-list screen — out of scope per the
earlier scope discussion (that list is short; adding it there is more
surface area for no clear benefit).

## 4. Shuffle — `PlayerActivity` only

New `Button` in `PlayerActivity`'s transport row, next to Previous/Next
(matching desktop's Prev/Play/Next/Shuffle ordering). Toggles Media3's
built-in `controller.shuffleModeEnabled` directly — chosen over a hand-rolled
reorder (like desktop's `shuffle_in_place`) because Media3 already solves
this: flipping one flag makes `seekToNextMediaItem`/`seekToPreviousMediaItem`
respect shuffle order for free. No new PRNG code needed on this side, unlike
desktop (which needed one because `rodio` has no such feature).

State survives across songs via one new field on the existing `Queue`
object (`Ui.kt`-adjacent singleton already used for the `Queue.pending`
handoff): `var shuffleEnabled: Boolean = false`. `PlayerActivity.connect()`
applies it to the controller right after `setMediaItems`/`prepare`, before
`play()`. The button's callback updates both the live controller
(immediate effect) and `Queue.shuffleEnabled` (so the next song tapped from
Library starts shuffled too).

## 5. Compact visual pass

Replace the current ad-hoc `"$title\n$artist"` packed into one `TextView`
(`LibraryActivity.darkAdapter`) with `android.R.layout.simple_list_item_2`
— a native two-line row layout — styled via `text1`/`text2`: bold
`Ui.FG` title, dim `Ui.FG_DIM` subtitle, alternating `Ui.BG`/`Ui.BG_ALT` row
background. Same visual language as desktop's track rows
(`ui/list.rs`'s bold-title/dim-subtitle-with-striping), zero new files,
still just `ArrayAdapter` + `ListView` — no `RecyclerView`, no new
dependency; `ListView`'s built-in view recycling already covers the
"stay lightweight at any library size" requirement the same way it does
today.

## Non-goals (explicitly out of scope this pass)

- No Loop mode on mobile — not requested, desktop's is a queue-order mode
  tied to `engine.rs`'s lookahead machinery that doesn't have a mobile
  analogue worth inventing speculatively.
- No search/sort on the playlist-list screen (see §3).
- No server-side search (`search3.view` etc.) — filtering stays client-side
  over the already-fetched list, exactly like desktop; adding a server
  endpoint would expand `server.rs` for no requested benefit.
- No Gradle dependency changes, no `Sync.kt`/`LocalLibrary.kt`/
  `PlaybackService.kt`/`Subsonic.kt` changes.

## Files touched

- `Ui.kt` — `topBar()`, inset padding in `column()`, two-line adapter
  helper.
- `LibraryActivity.kt` — top bar wiring (both states), search/sort state +
  `renderSongs()`, click handler now queues from displayed order.
- `PlayerActivity.kt` — top bar wiring, shuffle button, and one new field
  (`shuffleEnabled`) on the `Queue` object already declared at the top of
  this file.
- `PairActivity.kt` — top bar wiring (cosmetic only).
