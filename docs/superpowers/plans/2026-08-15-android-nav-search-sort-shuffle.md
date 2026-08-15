# Android nav/search/sort/shuffle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the camera-cutout clip, give every non-root Android screen an on-screen back affordance, and add search/sort (Library) and shuffle (Player) to the companion app — with zero new Gradle dependencies.

**Architecture:** All fixes live in `android/app/src/main/java/dev/melodie/companion/`. `Ui.kt` gains three shared building blocks (inset-aware `column()`, `topBar()`, `twoLineAdapter()`) that `LibraryActivity`, `PlayerActivity`, and `PairActivity` consume. Search/sort logic is extracted into a pure top-level function (`filterAndSortSongs`) so it's JUnit-testable on the JVM, mirroring how the desktop's `apply_view` (src/app.rs) was split out from FLTK widget code. Shuffle uses Media3's built-in `shuffleModeEnabled` — no custom reorder/RNG code.

**Tech Stack:** Kotlin, plain `android.widget.*` views (no AppCompat, no Compose, no RecyclerView), Media3 `MediaController`/`ExoPlayer` (already a dependency), JUnit4 (already a test dependency).

**Spec:** `docs/superpowers/specs/2026-08-15-android-nav-search-sort-shuffle-design.md`

## Global Constraints

- **Zero new Gradle dependencies** — every task uses either a plain platform API or the already-present Media3 APIs.
- **No `androidx.activity`/AppCompat** — `topBar`'s back control is a plain `TextView` with a click listener, not a `Toolbar`/`ActionBar`.
- **No `WindowInsets.Type` (API 30+) branching** — use the deprecated-but-functional `systemWindowInsetTop`/`systemWindowInsetBottom` fields so there's no `Build.VERSION.SDK_INT` check needed against `minSdk = 24`.
- **`minSdk = 24`, `compileSdk`/`targetSdk = 36`, `jvmTarget = 17`, Kotlin `2.2.21`** (`android/app/build.gradle.kts`, `android/build.gradle.kts`) — `SortField.entries` (Kotlin 1.9+ enum entries) is safe to use.
- All Gradle commands below run from `android/` (e.g. `cd android && ./gradlew ...`).

---

## File Structure

- **Modify `Ui.kt`** — add inset padding to `column()`, add `topBar()`, add `twoLineAdapter()`. Everything here is a shared, Android-framework-only helper — no business logic.
- **Modify `LibraryActivity.kt`** — add top-level `SortField` enum + `filterAndSortSongs()` pure function (new, testable); wire `topBar`/search/sort/`twoLineAdapter` into `showPlaylists()`/`showSongs()`; fix the tap-to-play queue to use the displayed (filtered/sorted) order.
- **New `LibraryActivityTest.kt`** (`android/app/src/test/java/dev/melodie/companion/`) — JUnit tests for `filterAndSortSongs()`.
- **Modify `PlayerActivity.kt`** — add `topBar`; add a Shuffle button next to Previous/Next; add `Queue.shuffleEnabled` and apply/toggle it against the `MediaController`.
- **Modify `PairActivity.kt`** — swap the plain title label for `Ui.topBar(..., onBack = null)` (cosmetic consistency only).

---

### Task 1: `Ui.kt` — fix the camera-cutout clip (inset padding in `column()`)

**Files:**
- Modify: `android/app/src/main/java/dev/melodie/companion/Ui.kt` (`column()`, currently lines 31-41)

**Interfaces:**
- Consumes: nothing new.
- Produces: `Ui.column(context: Context): LinearLayout` — same signature as today, now pads its top/bottom for the status bar / display cutout / nav bar automatically. Every other task in this plan builds on top of this (all three activities' root views come from `column()`).

This one isn't unit-testable (it's a `View.OnApplyWindowInsetsListener` registration — no Android framework under plain JUnit in this project, see `LocalLibraryTest.kt`/`SubsonicTest.kt` for the precedent that only framework-free code gets JVM tests here). Verify by compiling, then visually in Task 9's manual check.

- [ ] **Step 1: Replace `column()`**

Replace:

```kotlin
    fun column(context: Context): LinearLayout = LinearLayout(context).apply {
        orientation = LinearLayout.VERTICAL
        gravity = Gravity.CENTER_HORIZONTAL
        setBackgroundColor(BG)
        val p = dp(context, 16)
        setPadding(p, p, p, p)
        layoutParams = ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
    }
```

with:

```kotlin
    fun column(context: Context): LinearLayout = LinearLayout(context).apply {
        orientation = LinearLayout.VERTICAL
        gravity = Gravity.CENTER_HORIZONTAL
        setBackgroundColor(BG)
        val p = dp(context, 16)
        setPadding(p, p, p, p)
        layoutParams = ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
        // targetSdk 36 enforces edge-to-edge, so content draws under the
        // status bar / camera cutout / nav bar unless padded for it. Fixed
        // once here (every screen's root is `column()`) rather than per
        // activity. `systemWindowInsetTop/Bottom` (deprecated in API 30
        // favor of `WindowInsets.Type`) is used on purpose: it's been
        // functional since API 20, so there's no SDK_INT branch needed
        // against this app's minSdk 24.
        setOnApplyWindowInsetsListener { v, insets ->
            @Suppress("DEPRECATION")
            v.setPadding(p, p + insets.systemWindowInsetTop, p, p + insets.systemWindowInsetBottom)
            insets
        }
    }
```

- [ ] **Step 2: Compile**

Run: `cd android && ./gradlew :app:compileDebugKotlin`
Expected: `BUILD SUCCESSFUL`

- [ ] **Step 3: Commit**

```bash
git add android/app/src/main/java/dev/melodie/companion/Ui.kt
git commit -m "android: pad Ui.column for status bar / cutout / nav bar insets"
```

---

### Task 2: `Ui.kt` — `topBar()` helper

**Files:**
- Modify: `android/app/src/main/java/dev/melodie/companion/Ui.kt`

**Interfaces:**
- Consumes: `Context`, existing `Ui.FG`, `Ui.dp()`.
- Produces: `fun topBar(context: Context, title: String, onBack: (() -> Unit)?): LinearLayout` — a horizontal row: an optional "‹" back control (only present when `onBack != null`) plus a bold, single-line, ellipsized title. Tasks 5-8 all call this.

- [ ] **Step 1: Add two imports**

At the top of `Ui.kt`, add to the existing import block (alphabetical, matching the file's existing style):

```kotlin
import android.graphics.Typeface
import android.text.TextUtils
```

(Full resulting import block, for reference — `Context`, `Color`, `GradientDrawable`, `Gravity`, `Typeface`, `TextUtils`, `ViewGroup`, `Button`, `EditText`, `LinearLayout`, `TextView`, in that order.)

- [ ] **Step 2: Add `topBar()`**

Add this function to the `Ui` object, right after `column()`:

```kotlin
    /**
     * A back chevron (only when `onBack` is non-null) plus a bold title —
     * the hand-rolled stand-in for a `Toolbar`/`ActionBar`, which would
     * pull in `androidx.activity`/AppCompat. Every non-root screen gets
     * one so there's always an on-screen way back, not just gesture/system
     * back.
     */
    fun topBar(context: Context, title: String, onBack: (() -> Unit)?): LinearLayout =
        LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
            if (onBack != null) {
                addView(TextView(context).apply {
                    text = "‹" // ‹
                    textSize = 22f
                    setTextColor(FG)
                    gravity = Gravity.CENTER
                    minWidth = dp(context, 48)
                    minHeight = dp(context, 48)
                    isClickable = true
                    isFocusable = true
                    setOnClickListener { onBack() }
                })
            }
            addView(TextView(context).apply {
                text = title
                textSize = 18f
                setTypeface(typeface, Typeface.BOLD)
                setTextColor(FG)
                maxLines = 1
                ellipsize = TextUtils.TruncateAt.END
                val leftPad = if (onBack != null) 0 else dp(context, 12)
                setPadding(leftPad, dp(context, 8), dp(context, 12), dp(context, 8))
                layoutParams = LinearLayout.LayoutParams(
                    0,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                    1f,
                )
            })
        }
```

- [ ] **Step 3: Compile**

Run: `cd android && ./gradlew :app:compileDebugKotlin`
Expected: `BUILD SUCCESSFUL`

- [ ] **Step 4: Commit**

```bash
git add android/app/src/main/java/dev/melodie/companion/Ui.kt
git commit -m "android: add Ui.topBar() — back chevron + title, no Toolbar dependency"
```

---

### Task 3: `Ui.kt` — `twoLineAdapter()` helper

**Files:**
- Modify: `android/app/src/main/java/dev/melodie/companion/Ui.kt`

**Interfaces:**
- Consumes: existing `Ui.FG`, `Ui.FG_DIM`, `Ui.BG`, `Ui.BG_ALT`.
- Produces: `fun twoLineAdapter(context: Context, rows: List<Pair<String, String>>): ArrayAdapter<String>` — each `Pair` is (title, subtitle). Replaces `LibraryActivity`'s current `darkAdapter(rows: List<String>)`, which packed `"title\nsubtitle"` into one `TextView`. Task 5/6 are the two call sites.

- [ ] **Step 1: Add one import**

```kotlin
import android.widget.ArrayAdapter
```

- [ ] **Step 2: Add `twoLineAdapter()`**

Add to the `Ui` object, after `button()`:

```kotlin
    /**
     * Native two-line row (`simple_list_item_2`) styled to match the
     * desktop's bold-title/dim-subtitle track rows (`ui/list.rs`) —
     * replaces packing "title\nsubtitle" into one TextView. Still plain
     * `ArrayAdapter`/`ListView`, not `RecyclerView`: `ListView`'s built-in
     * view recycling already covers "stay lightweight at any library
     * size," so there's nothing a new dependency would buy here.
     */
    fun twoLineAdapter(context: Context, rows: List<Pair<String, String>>): ArrayAdapter<String> =
        object : ArrayAdapter<String>(
            context,
            android.R.layout.simple_list_item_2,
            android.R.id.text1,
            rows.map { it.first },
        ) {
            override fun getView(position: Int, convertView: android.view.View?, parent: ViewGroup): android.view.View {
                val view = super.getView(position, convertView, parent)
                view.findViewById<TextView>(android.R.id.text1).apply {
                    text = rows[position].first
                    setTextColor(FG)
                }
                view.findViewById<TextView>(android.R.id.text2).apply {
                    text = rows[position].second
                    setTextColor(FG_DIM)
                }
                view.setBackgroundColor(if (position % 2 == 0) BG else BG_ALT)
                return view
            }
        }
```

- [ ] **Step 3: Compile**

Run: `cd android && ./gradlew :app:compileDebugKotlin`
Expected: `BUILD SUCCESSFUL`

- [ ] **Step 4: Commit**

```bash
git add android/app/src/main/java/dev/melodie/companion/Ui.kt
git commit -m "android: add Ui.twoLineAdapter() — native two-line rows matching desktop styling"
```

---

### Task 4: `LibraryActivity.kt` — `SortField` + `filterAndSortSongs()` (TDD)

**Files:**
- Modify: `android/app/src/main/java/dev/melodie/companion/LibraryActivity.kt` (add near the top, above `class LibraryActivity`)
- Test: `android/app/src/test/java/dev/melodie/companion/LibraryActivityTest.kt` (new)

**Interfaces:**
- Consumes: `RemoteSong` (`Subsonic.kt`, already has `title`/`artist`/`album`/`durationSec`).
- Produces: `enum class SortField { TITLE, ARTIST, ALBUM, DURATION }` and `fun filterAndSortSongs(songs: List<RemoteSong>, query: String, sort: SortField): List<RemoteSong>`. Task 6 is the consumer.

- [ ] **Step 1: Write the failing test**

Create `android/app/src/test/java/dev/melodie/companion/LibraryActivityTest.kt`:

```kotlin
package dev.melodie.companion

import org.junit.Assert.assertEquals
import org.junit.Test

class LibraryActivityTest {

    private fun song(id: String, title: String, artist: String, album: String, durationSec: Int) =
        RemoteSong(id, title, artist, album, durationSec, "mp3")

    private val songs = listOf(
        song("1", "Beta Track", "Artist Two", "Album B", 200),
        song("2", "Alpha Song", "Artist One", "Album A", 100),
        song("3", "Gamma Tune", "Artist One", "Album C", 50),
    )

    @Test
    fun filtersCaseInsensitivelyAcrossTitleArtistAlbum() {
        val got = filterAndSortSongs(songs, "one", SortField.TITLE).map { it.id }
        assertEquals(listOf("2", "3"), got) // both have "Artist One"
    }

    @Test
    fun sortsByEachField() {
        assertEquals(listOf("2", "1", "3"), filterAndSortSongs(songs, "", SortField.TITLE).map { it.id })
        assertEquals(listOf("3", "2", "1"), filterAndSortSongs(songs, "", SortField.DURATION).map { it.id })
    }

    @Test
    fun blankQueryReturnsEverything() {
        assertEquals(3, filterAndSortSongs(songs, "", SortField.TITLE).size)
    }

    @Test
    fun noMatchesReturnsEmptyList() {
        assertEquals(emptyList<RemoteSong>(), filterAndSortSongs(songs, "zzz", SortField.TITLE))
    }
}
```

- [ ] **Step 2: Run it, confirm it fails to compile (the function doesn't exist yet)**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests "dev.melodie.companion.LibraryActivityTest"`
Expected: FAIL — compile error, `Unresolved reference: filterAndSortSongs` (and `SortField`).

- [ ] **Step 3: Add `SortField` and `filterAndSortSongs()`**

At the top of `LibraryActivity.kt`, after the `package`/`import` lines and before `class LibraryActivity`, add:

```kotlin
enum class SortField { TITLE, ARTIST, ALBUM, DURATION }

/**
 * Filters `songs` by substring across title/artist/album (case-insensitive),
 * then sorts by `sort`. Pure and framework-free on purpose — testable on
 * the JVM, mirroring the desktop's `apply_view` (src/app.rs).
 */
fun filterAndSortSongs(songs: List<RemoteSong>, query: String, sort: SortField): List<RemoteSong> {
    val filtered = if (query.isBlank()) {
        songs
    } else {
        val needle = query.lowercase()
        songs.filter {
            it.title.lowercase().contains(needle) ||
                it.artist.lowercase().contains(needle) ||
                it.album.lowercase().contains(needle)
        }
    }
    return when (sort) {
        SortField.TITLE -> filtered.sortedBy { it.title.lowercase() }
        SortField.ARTIST -> filtered.sortedBy { it.artist.lowercase() }
        SortField.ALBUM -> filtered.sortedBy { it.album.lowercase() }
        SortField.DURATION -> filtered.sortedBy { it.durationSec }
    }
}
```

- [ ] **Step 4: Run the test again, confirm it passes**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests "dev.melodie.companion.LibraryActivityTest"`
Expected: `BUILD SUCCESSFUL`, 4 tests passed.

- [ ] **Step 5: Commit**

```bash
git add android/app/src/main/java/dev/melodie/companion/LibraryActivity.kt android/app/src/test/java/dev/melodie/companion/LibraryActivityTest.kt
git commit -m "android: add filterAndSortSongs() with unit tests"
```

---

### Task 5: `LibraryActivity.kt` — top bar + `twoLineAdapter` on the playlist list

**Files:**
- Modify: `android/app/src/main/java/dev/melodie/companion/LibraryActivity.kt` (class fields, `onCreate`, `showPlaylists`, delete `darkAdapter`)

**Interfaces:**
- Consumes: `Ui.topBar()` (Task 2), `Ui.twoLineAdapter()` (Task 3).
- Produces: `private fun setTopBar(title: String, onBack: (() -> Unit)?)` — Task 6 also calls this from `showSongs`.

- [ ] **Step 1: Add a `topBarView` field**

Find the existing field block:

```kotlin
    private var playlists: List<RemotePlaylist> = emptyList()
    private var openPlaylist: RemotePlaylist? = null
    private var songs: List<RemoteSong> = emptyList()
```

Add right after it:

```kotlin
    private var topBarView: LinearLayout? = null
```

- [ ] **Step 2: Add `setTopBar()`**

Add this private method anywhere in the class (e.g. right after `onBackPressed`):

```kotlin
    private fun setTopBar(title: String, onBack: (() -> Unit)?) {
        topBarView?.let { root.removeView(it) }
        val bar = Ui.topBar(this, title, onBack)
        root.addView(bar, 0)
        topBarView = bar
    }
```

- [ ] **Step 3: Replace the static title label in `onCreate`**

Replace:

```kotlin
        root.addView(Ui.label(this, "Melodie", 22f))
        root.addView(status)
        root.addView(list)
        setContentView(root)
```

with:

```kotlin
        setTopBar("Melodie", null)
        root.addView(status)
        root.addView(list)
        setContentView(root)
```

- [ ] **Step 4: Rewrite `showPlaylists()` to use the top bar and `twoLineAdapter`**

Replace:

```kotlin
    private fun showPlaylists() {
        openPlaylist = null
        syncButtonView?.let { root.removeView(it) }
        syncButtonView = null
        val local = LocalLibrary.load(filesDir).groupBy { it.playlist }
        val rows = playlists.map { p ->
            val synced = local[p.name]?.size ?: 0
            if (synced > 0) "${p.name}\n$synced of ${p.songCount} on this phone"
            else "${p.name}\n${p.songCount} tracks — not synced"
        }
        status.text = if (playlists.isEmpty()) "No playlists on the server yet." else "Tap a playlist."
        list.adapter = darkAdapter(rows)
        list.setOnItemClickListener { _, _, index, _ -> openPlaylist(playlists[index]) }
    }
```

with:

```kotlin
    private fun showPlaylists() {
        openPlaylist = null
        setTopBar("Melodie", null)
        syncButtonView?.let { root.removeView(it) }
        syncButtonView = null
        searchInput?.let { root.removeView(it) }
        searchInput = null
        sortSpinner?.let { root.removeView(it) }
        sortSpinner = null
        val local = LocalLibrary.load(filesDir).groupBy { it.playlist }
        val rows = playlists.map { p ->
            val synced = local[p.name]?.size ?: 0
            val subtitle = if (synced > 0) "$synced of ${p.songCount} on this phone" else "${p.songCount} tracks — not synced"
            p.name to subtitle
        }
        status.text = if (playlists.isEmpty()) "No playlists on the server yet." else "Tap a playlist."
        list.adapter = Ui.twoLineAdapter(this, rows)
        list.setOnItemClickListener { _, _, index, _ -> openPlaylist(playlists[index]) }
    }
```

(`searchInput`/`sortSpinner` fields are added in Task 6 — this step references them because `showPlaylists()` and Task 6's song-list code share cleanup responsibility. If executing Task 5 before Task 6, this line will not yet compile; do Task 5 and Task 6 in the same sitting, or add the two fields — `private var searchInput: EditText? = null` and `private var sortSpinner: Spinner? = null` — as part of this step instead of waiting for Task 6.)

- [ ] **Step 5: Delete the now-unused `darkAdapter`**

Delete:

```kotlin
    private fun darkAdapter(rows: List<String>): ArrayAdapter<String> =
        object : ArrayAdapter<String>(this, android.R.layout.simple_list_item_1, rows) {
            override fun getView(position: Int, convertView: android.view.View?, parent: android.view.ViewGroup): android.view.View {
                val view = super.getView(position, convertView, parent)
                (view as android.widget.TextView).setTextColor(Ui.FG)
                return view
            }
        }
```

- [ ] **Step 6: Compile**

Run: `cd android && ./gradlew :app:compileDebugKotlin`
Expected: fails here if the `searchInput`/`sortSpinner` fields from Task 6 weren't added yet — add them now (see Step 4's note) so this compiles standalone:

```kotlin
    private var searchInput: EditText? = null
    private var sortSpinner: Spinner? = null
```

placed next to the `topBarView` field from Step 1. `EditText` is already imported implicitly via `Ui.input()`'s return type — add the explicit imports now so this compiles:

```kotlin
import android.widget.EditText
import android.widget.Spinner
```

Run: `cd android && ./gradlew :app:compileDebugKotlin`
Expected: `BUILD SUCCESSFUL`

- [ ] **Step 7: Commit**

```bash
git add android/app/src/main/java/dev/melodie/companion/LibraryActivity.kt
git commit -m "android: wire Ui.topBar/twoLineAdapter into the playlist list screen"
```

---

### Task 6: `LibraryActivity.kt` — search/sort + top bar on the song-list screen, fix queue ordering

**Files:**
- Modify: `android/app/src/main/java/dev/melodie/companion/LibraryActivity.kt` (`showSongs`, `startSync`'s `onDone`, new `renderSongs`/`setupSearchAndSortBar`)

**Interfaces:**
- Consumes: `filterAndSortSongs()`/`SortField` (Task 4), `Ui.twoLineAdapter()` (Task 3), `setTopBar()` (Task 5).
- Produces: `private fun renderSongs(playlist: RemotePlaylist)` — the single place the song list's rows, status text, sync button, and tap-to-play ordering get built. Nothing outside this file consumes it.

- [ ] **Step 1: Add search/sort imports**

Add to `LibraryActivity.kt`'s imports:

```kotlin
import android.text.Editable
import android.text.TextWatcher
import android.widget.AdapterView
import android.widget.ArrayAdapter
```

(`EditText`/`Spinner` were already added in Task 5 Step 6.)

- [ ] **Step 2: Add `searchQuery`/`sortField` state fields**

Next to the fields added in Task 5, add:

```kotlin
    private var searchQuery: String = ""
    private var sortField: SortField = SortField.TITLE
```

- [ ] **Step 3: Replace `showSongs()` with an entry function + `renderSongs()`**

Replace the whole current `showSongs`:

```kotlin
    private fun showSongs(playlist: RemotePlaylist) {
        val local = LocalLibrary.load(filesDir).filter { it.playlist == playlist.name }
        val haveIds = local.map { it.id }.toSet()
        val rows = songs.map { s ->
            val mark = if (s.id in haveIds) "" else "  (not synced)"
            "${s.title}\n${s.artist}$mark"
        }
        status.text = "${playlist.name} — ${haveIds.size}/${songs.size} on this phone"
        list.adapter = darkAdapter(rows)
        list.setOnItemClickListener { _, _, index, _ ->
            val song = songs[index]
            val localSong = local.firstOrNull { it.id == song.id }
            if (localSong == null) {
                Toast.makeText(this, "Sync this playlist first", Toast.LENGTH_SHORT).show()
            } else {
                val ordered = songs.mapNotNull { s -> local.firstOrNull { it.id == s.id } }
                val start = ordered.indexOfFirst { it.id == song.id }
                startPlayback(ordered, if (start < 0) 0 else start)
            }
        }
        // No RemotePlaylist.id to sync against for an offline entry — there's
        // nothing a Sync button could do here that hasn't already happened.
        if (playlist.id.isEmpty()) {
            syncButtonView?.let { root.removeView(it) }
            syncButtonView = null
        } else {
            syncButton(playlist)
        }
    }
```

with:

```kotlin
    /**
     * Entry point when a playlist is tapped: resets search/sort, builds the
     * top bar and the search/sort controls once, then renders. `startSync`'s
     * completion callback calls `renderSongs` directly instead (search text
     * and sort choice shouldn't reset just because a sync finished).
     */
    private fun showSongs(playlist: RemotePlaylist) {
        searchQuery = ""
        sortField = SortField.TITLE
        setTopBar(playlist.name) { showPlaylists() }
        setupSearchAndSortBar(playlist)
        renderSongs(playlist)
    }

    private fun setupSearchAndSortBar(playlist: RemotePlaylist) {
        searchInput?.let { root.removeView(it) }
        sortSpinner?.let { root.removeView(it) }

        val input = Ui.input(this, "Search songs")
        input.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {
                searchQuery = s?.toString() ?: ""
                renderSongs(playlist)
            }
            override fun afterTextChanged(s: Editable?) {}
        })

        val spinner = Spinner(this).apply {
            adapter = ArrayAdapter(
                this@LibraryActivity,
                android.R.layout.simple_spinner_item,
                listOf("Title", "Artist", "Album", "Duration"),
            ).apply { setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item) }
            onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(parent: AdapterView<*>?, view: android.view.View?, position: Int, id: Long) {
                    sortField = SortField.entries[position]
                    renderSongs(playlist)
                }
                override fun onNothingSelected(parent: AdapterView<*>?) {}
            }
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = Ui.dp(this@LibraryActivity, 4) }
        }

        // Inserted right below the top bar (index 0), above `status`/`list`
        // — `showPlaylists()` always removes these two before this is ever
        // called again, so `root`'s children are reliably
        // [topBar, status, list] (indices 0-2) at this point.
        root.addView(input, 1)
        root.addView(spinner, 2)
        searchInput = input
        sortSpinner = spinner
    }

    /**
     * The single place the song list's rows, status text, sync button, and
     * tap-to-play ordering get built from `songs` + the current
     * search/sort state. Queuing from the *displayed* (filtered/sorted)
     * order — not the server's raw order — is the fix: previously a tap
     * always queued from `songs` even though nothing kept that in sync with
     * what was on screen.
     */
    private fun renderSongs(playlist: RemotePlaylist) {
        val local = LocalLibrary.load(filesDir).filter { it.playlist == playlist.name }
        val haveIds = local.map { it.id }.toSet()
        val displayed = filterAndSortSongs(songs, searchQuery, sortField)
        val rows = displayed.map { s ->
            val subtitle = if (s.id in haveIds) s.artist else "${s.artist}  (not synced)"
            s.title to subtitle
        }
        status.text = "${playlist.name} — ${haveIds.size}/${songs.size} on this phone"
        list.adapter = Ui.twoLineAdapter(this, rows)
        list.setOnItemClickListener { _, _, index, _ ->
            val song = displayed[index]
            val localSong = local.firstOrNull { it.id == song.id }
            if (localSong == null) {
                Toast.makeText(this, "Sync this playlist first", Toast.LENGTH_SHORT).show()
            } else {
                val ordered = displayed.mapNotNull { s -> local.firstOrNull { it.id == s.id } }
                val start = ordered.indexOfFirst { it.id == song.id }
                startPlayback(ordered, if (start < 0) 0 else start)
            }
        }
        if (playlist.id.isEmpty()) {
            syncButtonView?.let { root.removeView(it) }
            syncButtonView = null
        } else {
            syncButton(playlist)
        }
    }
```

- [ ] **Step 4: Point `startSync`'s completion callback at `renderSongs`, not `showSongs`**

In `startSync`, replace:

```kotlin
            onDone = { synced, failed ->
                main.post {
                    status.text = if (failed == 0) {
                        "${playlist.name}: $synced tracks on this phone"
                    } else {
                        "${playlist.name}: $synced synced, $failed failed"
                    }
                    showSongs(playlist)
                }
            },
```

with:

```kotlin
            onDone = { synced, failed ->
                main.post {
                    status.text = if (failed == 0) {
                        "${playlist.name}: $synced tracks on this phone"
                    } else {
                        "${playlist.name}: $synced synced, $failed failed"
                    }
                    renderSongs(playlist)
                }
            },
```

(This preserves whatever search text/sort field the user had set while a sync was running — a `showSongs` call there would have reset both back to defaults.)

- [ ] **Step 5: Compile and run tests**

Run: `cd android && ./gradlew :app:compileDebugKotlin :app:testDebugUnitTest`
Expected: `BUILD SUCCESSFUL`, all tests (including Task 4's) pass.

- [ ] **Step 6: Commit**

```bash
git add android/app/src/main/java/dev/melodie/companion/LibraryActivity.kt
git commit -m "android: search/sort on the song list, queue from displayed order"
```

---

### Task 7: `PlayerActivity.kt` — top bar + shuffle

**Files:**
- Modify: `android/app/src/main/java/dev/melodie/companion/PlayerActivity.kt` (`Queue` object, `onCreate`, `connect`)

**Interfaces:**
- Consumes: `Ui.topBar()` (Task 2).
- Produces: `Queue.shuffleEnabled: Boolean` — read by `connect()` on every (re)connection; no other file reads or writes it.

- [ ] **Step 1: Add `shuffleEnabled` to `Queue`**

Replace:

```kotlin
object Queue {
    var pending: List<LocalSong> = emptyList()
}
```

with:

```kotlin
object Queue {
    var pending: List<LocalSong> = emptyList()
    /** Mirrors the last Shuffle toggle so the next queued playlist starts shuffled too. */
    var shuffleEnabled: Boolean = false
}
```

- [ ] **Step 2: Add a `shuffleBtn` field**

Replace:

```kotlin
    private lateinit var nowPlaying: TextView
    private lateinit var position: TextView
    private lateinit var seek: SeekBar
    private lateinit var playPause: android.widget.Button
    private var userIsSeeking = false
```

with:

```kotlin
    private lateinit var nowPlaying: TextView
    private lateinit var position: TextView
    private lateinit var seek: SeekBar
    private lateinit var playPause: android.widget.Button
    private lateinit var shuffleBtn: android.widget.Button
    private var userIsSeeking = false
```

- [ ] **Step 3: Add the top bar and the Shuffle button in `onCreate`**

Replace:

```kotlin
        playPause = Ui.button(this, "Play") { toggle() }
        root.addView(nowPlaying)
        root.addView(position)
        root.addView(seek)
        root.addView(playPause)
        root.addView(Ui.button(this, "Previous") { controller?.seekToPreviousMediaItem() })
        root.addView(Ui.button(this, "Next") { controller?.seekToNextMediaItem() })
        setContentView(root)
```

with:

```kotlin
        playPause = Ui.button(this, "Play") { toggle() }
        shuffleBtn = Ui.button(this, "Shuffle") { toggleShuffle() }
        root.addView(Ui.topBar(this, "Now Playing") { finish() })
        root.addView(nowPlaying)
        root.addView(position)
        root.addView(seek)
        root.addView(playPause)
        root.addView(Ui.button(this, "Previous") { controller?.seekToPreviousMediaItem() })
        root.addView(Ui.button(this, "Next") { controller?.seekToNextMediaItem() })
        root.addView(shuffleBtn)
        setContentView(root)
        updateShuffleColor()
```

- [ ] **Step 4: Apply `Queue.shuffleEnabled` on every connect, and add the toggle/color helpers**

In `connect()`, replace:

```kotlin
            controller = c
            val queue = Queue.pending
            if (queue.isNotEmpty()) {
                Queue.pending = emptyList()
                c.setMediaItems(queue.map { toMediaItem(it) }, startIndex, 0L)
                c.prepare()
                c.play()
            }
            main.post(tick)
```

with:

```kotlin
            controller = c
            c.shuffleModeEnabled = Queue.shuffleEnabled
            val queue = Queue.pending
            if (queue.isNotEmpty()) {
                Queue.pending = emptyList()
                c.setMediaItems(queue.map { toMediaItem(it) }, startIndex, 0L)
                c.prepare()
                c.play()
            }
            updateShuffleColor()
            main.post(tick)
```

Then add these two methods anywhere in the class (e.g. right after `toggle()`):

```kotlin
    /**
     * Toggles Media3's built-in shuffle rather than reordering the queue
     * ourselves (unlike desktop's `shuffle_in_place`, src/app.rs) —
     * ExoPlayer/MediaController already makes seekToNext/PreviousMediaItem
     * respect shuffle order once this flag is set.
     */
    private fun toggleShuffle() {
        Queue.shuffleEnabled = !Queue.shuffleEnabled
        controller?.shuffleModeEnabled = Queue.shuffleEnabled
        updateShuffleColor()
    }

    private fun updateShuffleColor() {
        (shuffleBtn.background as? android.graphics.drawable.GradientDrawable)?.setColor(
            if (Queue.shuffleEnabled) Ui.ACCENT else Ui.BG_ALT
        )
    }
```

- [ ] **Step 5: Compile**

Run: `cd android && ./gradlew :app:compileDebugKotlin`
Expected: `BUILD SUCCESSFUL`

- [ ] **Step 6: Commit**

```bash
git add android/app/src/main/java/dev/melodie/companion/PlayerActivity.kt
git commit -m "android: add top bar and Media3 native shuffle to PlayerActivity"
```

---

### Task 8: `PairActivity.kt` — top bar (cosmetic)

**Files:**
- Modify: `android/app/src/main/java/dev/melodie/companion/PairActivity.kt`

**Interfaces:**
- Consumes: `Ui.topBar()` (Task 2).
- Produces: nothing new — visual consistency only.

- [ ] **Step 1: Swap the label for a top bar**

Replace:

```kotlin
        val root = Ui.column(this)
        root.addView(Ui.label(this, "Pair with Melodie", 22f))
```

with:

```kotlin
        val root = Ui.column(this)
        root.addView(Ui.topBar(this, "Pair with Melodie", null))
```

- [ ] **Step 2: Compile**

Run: `cd android && ./gradlew :app:compileDebugKotlin`
Expected: `BUILD SUCCESSFUL`

- [ ] **Step 3: Commit**

```bash
git add android/app/src/main/java/dev/melodie/companion/PairActivity.kt
git commit -m "android: use Ui.topBar on the pairing screen for visual consistency"
```

---

### Task 9: Full build + manual verification

**Files:** none (verification only).

**Interfaces:** none — this task consumes everything above and produces a signed-off, running APK.

- [ ] **Step 1: Full test suite**

Run: `cd android && ./gradlew :app:testDebugUnitTest`
Expected: `BUILD SUCCESSFUL`, all tests pass (`SubsonicTest`, `LocalLibraryTest`, `LibraryActivityTest`).

- [ ] **Step 2: Assemble the debug APK**

Run: `cd android && ./gradlew assembleDebug`
Expected: `BUILD SUCCESSFUL`.

- [ ] **Step 3: Install and manually verify on a device/emulator**

Run: `cd android && ./gradlew installDebug`

Checklist (there is no instrumentation-test setup in this project — `android/app/src/test` is JVM-only, per the existing `SubsonicTest`/`LocalLibraryTest`, so this is the project's established way of checking Activity/View wiring):

- [ ] Launch the app on a device with a display cutout (or check the top edge on any device): no text/controls clipped under the status bar or camera cutout, on both the Pair screen and Library.
- [ ] From the playlist list, tap into a playlist: a "‹ [playlist name]" top bar appears; tapping "‹" returns to the playlist list.
- [ ] From a song list, type in the search box: the list filters live across title/artist/album, case-insensitively.
- [ ] Change the sort spinner through all four options: the list re-orders accordingly.
- [ ] Tap a synced song while a search filter is active: playback starts from that song, and Next/Previous walk the *filtered* list, not the full playlist.
- [ ] Start playback, open the Player screen: a "‹ Now Playing" top bar appears; tapping "‹" returns to the song list (not just system back).
- [ ] Tap Shuffle: the button's fill color changes (ACCENT when on); Next/Previous now advance in shuffled order.
- [ ] Return to Library, open a different playlist, tap a song: it also starts shuffled (Shuffle state carried over via `Queue.shuffleEnabled`).
- [ ] Tap Shuffle again to turn it off; confirm Next/Previous return to in-order.

- [ ] **Step 4: Record the result**

If every checklist item passes, this plan is complete. If something fails, fix it as a follow-up commit against the specific task above it belongs to — don't bundle unrelated fixes into this task.

---

## Self-Review Notes

- **Spec coverage:** §1 (insets) → Task 1. §2 (topBar, all 4 screens) → Tasks 2, 5, 6, 7, 8. §3 (search/sort + queue-order fix) → Tasks 4, 6. §4 (shuffle) → Task 7. §5 (two-line rows) → Tasks 3, 5, 6. Non-goals section requires no tasks (nothing to build).
- **Placeholder scan:** no TBD/TODO; every step has literal code, not a description of code.
- **Type consistency:** `SortField` (Task 4) has 4 entries (`TITLE`/`ARTIST`/`ALBUM`/`DURATION`) matched 1:1 against the Spinner's 4-item list (Task 6) via `SortField.entries[position]`. `filterAndSortSongs(songs: List<RemoteSong>, query: String, sort: SortField): List<RemoteSong>` (Task 4) is called with exactly that signature in `renderSongs()` (Task 6). `Ui.topBar(context: Context, title: String, onBack: (() -> Unit)?)` (Task 2) is called consistently across Tasks 5/6/7/8 — `null` for root screens, a lambda for non-root ones. `Ui.twoLineAdapter(context: Context, rows: List<Pair<String, String>>)` (Task 3) is called with `List<Pair<String, String>>` at both call sites (Tasks 5, 6).
