package dev.melodie.companion

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.Editable
import android.text.TextWatcher
import android.widget.AdapterView
import android.widget.ArrayAdapter
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ListView
import android.widget.Spinner
import android.widget.Toast

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

/**
 * One screen, two modes: the server's playlists, and the tracks inside the
 * one you tapped. Playlists are synced individually — mirroring the whole
 * library onto a phone is rarely what anyone wants.
 */
class LibraryActivity : Activity() {

    private val main = Handler(Looper.getMainLooper())
    private lateinit var server: Server
    private lateinit var client: SubsonicClient
    private lateinit var root: LinearLayout
    private lateinit var status: android.widget.TextView
    private lateinit var list: ListView

    private var playlists: List<RemotePlaylist> = emptyList()
    private var openPlaylist: RemotePlaylist? = null
    private var songs: List<RemoteSong> = emptyList()
    private var topBarView: LinearLayout? = null
    private var searchInput: EditText? = null
    private var sortSpinner: Spinner? = null
    private var searchQuery: String = ""
    private var sortField: SortField = SortField.TITLE

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val paired = Pairing.load(this)
        if (paired == null) {
            startActivity(Intent(this, PairActivity::class.java))
            finish()
            return
        }
        server = paired
        client = SubsonicClient(server)

        root = Ui.column(this)
        status = Ui.dim(this, "Loading…")
        list = ListView(this).apply {
            setBackgroundColor(Ui.BG)
            divider = null
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                0,
            ).apply { weight = 1f }
        }
        setTopBar("Melodie", null)
        root.addView(status)
        root.addView(list)
        setContentView(root)

        // The media notification needs this on Android 13+; without it
        // playback still works but shows no controls.
        if (Build.VERSION.SDK_INT >= 33) {
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        }

        loadPlaylists()
    }

    override fun onBackPressed() {
        if (openPlaylist != null) showPlaylists() else super.onBackPressed()
    }

    private fun setTopBar(title: String, onBack: (() -> Unit)?) {
        topBarView?.let { root.removeView(it) }
        val bar = Ui.topBar(this, title, onBack)
        root.addView(bar, 0)
        topBarView = bar
    }

    private fun loadPlaylists() {
        status.text = "Loading playlists…"
        Thread {
            val result = runCatching { client.playlists() }
            main.post {
                result.onSuccess {
                    repairButtonView?.let { root.removeView(it) }
                    repairButtonView = null
                    playlists = it
                    showPlaylists()
                }.onFailure { e ->
                    showOfflineFallback(e)
                }
            }
        }.start()
    }

    /**
     * The core offline-first requirement: a server that's unreachable (no
     * Wi-Fi, rotated LAN IP, desktop off) must not strand tracks that are
     * already synced to this phone. Falls back to whatever LocalLibrary
     * already has, synthesizing playlist entries with an empty id — an
     * empty id is how the rest of this screen recognizes "this came from
     * the phone, not the server" and skips network calls for it.
     */
    private fun showOfflineFallback(e: Throwable) {
        val local = LocalLibrary.load(filesDir)
        if (local.isEmpty()) {
            status.text = "Can't reach Melodie: ${e.message}"
            showRepairButton()
            return
        }
        val names = local.map { it.playlist }.distinct()
        playlists = names.map { name ->
            RemotePlaylist(id = "", name = name, songCount = local.count { it.playlist == name })
        }
        showPlaylists()
        status.text = "Offline — showing what's on this phone."
        showRepairButton()
    }

    private var repairButtonView: android.widget.Button? = null

    /** Only reachable from the failure path — forgetting pairing otherwise requires clearing app data. */
    private fun showRepairButton() {
        if (repairButtonView != null) return
        val button = Ui.button(this, "Re-pair with a different server") {
            Pairing.clear(this)
            startActivity(Intent(this, PairActivity::class.java))
            finish()
        }
        repairButtonView = button
        root.addView(button)
    }

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

    private fun openPlaylist(playlist: RemotePlaylist) {
        openPlaylist = playlist
        // Empty id marks an offline-synthesized playlist (see
        // showOfflineFallback) — nothing to fetch, build the track list
        // straight from what's already on the phone.
        if (playlist.id.isEmpty()) {
            val local = LocalLibrary.load(filesDir).filter { it.playlist == playlist.name }
            songs = local.map { RemoteSong(it.id, it.title, it.artist, it.album, it.durationSec, "") }
            showSongs(playlist)
            return
        }
        status.text = "Loading ${playlist.name}…"
        Thread {
            val result = runCatching { client.playlist(playlist.id) }
            main.post {
                result.onSuccess {
                    songs = it
                    showSongs(playlist)
                }.onFailure { e ->
                    status.text = "Couldn't load ${playlist.name}: ${e.message}"
                }
            }
        }.start()
    }

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

    private var syncButtonView: android.widget.Button? = null

    private fun syncButton(playlist: RemotePlaylist) {
        syncButtonView?.let { root.removeView(it) }
        val button = Ui.button(this, "Sync \"${playlist.name}\" to this phone") {
            startSync(playlist)
        }
        syncButtonView = button
        root.addView(button)
    }

    private fun startSync(playlist: RemotePlaylist) {
        val queued = songs
        if (queued.isEmpty()) return
        status.text = "Syncing ${playlist.name}…"
        val started = Sync.playlist(
            filesDir = filesDir,
            client = client,
            playlistName = playlist.name,
            songs = queued,
            onProgress = { done, total, title ->
                main.post { status.text = "Syncing ${done + 1}/$total — $title" }
            },
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
        )
        if (!started) {
            Toast.makeText(this, "Sync already in progress", Toast.LENGTH_SHORT).show()
        }
    }

    private fun startPlayback(queue: List<LocalSong>, index: Int) {
        Queue.pending = queue
        startActivity(
            Intent(this, PlayerActivity::class.java)
                .putExtra(PlayerActivity.EXTRA_INDEX, index)
        )
    }
}
