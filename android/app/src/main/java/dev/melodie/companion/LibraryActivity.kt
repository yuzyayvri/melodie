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

    // Checked/set only on the main thread (startSync only ever runs from a
    // button click), so a plain Boolean is enough to stop a double-tap or a
    // second playlist's Sync button from racing library.tsv with this one.
    private var syncInFlight = false

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
                    playlists = it
                    showPlaylists()
                }.onFailure { e ->
                    status.text = "Can't reach Melodie: ${e.message}"
                }
            }
        }.start()
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
        syncButton(playlist)
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
        if (syncInFlight) {
            Toast.makeText(this, "Sync already in progress", Toast.LENGTH_SHORT).show()
            return
        }
        val queued = songs
        if (queued.isEmpty()) return
        syncInFlight = true
        status.text = "Syncing ${playlist.name}…"
        Sync.playlist(
            filesDir = filesDir,
            client = client,
            playlistName = playlist.name,
            songs = queued,
            onProgress = { done, total, title ->
                main.post { status.text = "Syncing ${done + 1}/$total — $title" }
            },
            onDone = { synced, failed ->
                main.post {
                    syncInFlight = false
                    status.text = if (failed == 0) {
                        "${playlist.name}: $synced tracks on this phone"
                    } else {
                        "${playlist.name}: $synced synced, $failed failed"
                    }
                    showSongs(playlist)
                }
            },
        )
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
