package dev.melodie.companion

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.ArrayAdapter
import android.widget.LinearLayout
import android.widget.ListView
import android.widget.Toast

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
        root.addView(Ui.label(this, "Melodie", 22f))
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
                    showSongs(playlist)
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

    private fun darkAdapter(rows: List<String>): ArrayAdapter<String> =
        object : ArrayAdapter<String>(this, android.R.layout.simple_list_item_1, rows) {
            override fun getView(position: Int, convertView: android.view.View?, parent: android.view.ViewGroup): android.view.View {
                val view = super.getView(position, convertView, parent)
                (view as android.widget.TextView).setTextColor(Ui.FG)
                return view
            }
        }
}
