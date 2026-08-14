package dev.melodie.companion

import java.io.File

/**
 * Downloads a playlist's tracks into app-private storage, one at a time.
 *
 * Serial on purpose, mirroring the desktop's own fetch policy (PLAN.md
 * §5.5): a personal library is not a race, and one socket at a time keeps
 * both memory and the phone's radio behaviour predictable.
 */
object Sync {

    fun playlist(
        filesDir: File,
        client: SubsonicClient,
        playlistName: String,
        songs: List<RemoteSong>,
        onProgress: (done: Int, total: Int, title: String) -> Unit,
        onDone: (synced: Int, failed: Int) -> Unit,
    ) {
        Thread {
            val existing = LocalLibrary.load(filesDir).toMutableList()
            val tracks = LocalLibrary.tracksDir(filesDir)
            var failed = 0

            songs.forEachIndexed { index, song ->
                onProgress(index, songs.size, song.title)
                val fileName = "${song.id}.${song.suffix}"
                val target = File(tracks, fileName)
                val entry = LocalSong(
                    id = song.id,
                    title = song.title,
                    artist = song.artist,
                    album = song.album,
                    durationSec = song.durationSec,
                    fileName = fileName,
                    playlist = playlistName,
                )

                val alreadyHere = target.exists() && target.length() > 0
                if (!alreadyHere) {
                    val result = runCatching { client.downloadTo(song.id, target) { } }
                    if (result.isFailure) {
                        failed++
                        return@forEachIndexed
                    }
                }
                // Re-syncing a playlist must not duplicate rows.
                existing.removeAll { it.id == song.id && it.playlist == playlistName }
                existing.add(entry)
                // Flush after every track, not just at the end: if the app
                // dies mid-playlist, tracks already downloaded to disk must
                // still show up as playable rather than sitting orphaned
                // until a full resync self-heals them.
                LocalLibrary.save(filesDir, existing)
            }

            // Tracks dropped from the playlist upstream lose their manifest
            // row; the file itself is left alone. Deleting a user's audio
            // because a remote list changed is the wrong default (PLAN.md
            // §5.3 makes the same call on the desktop).
            val keep = songs.map { it.id }.toSet()
            existing.removeAll { it.playlist == playlistName && it.id !in keep }

            LocalLibrary.save(filesDir, existing)
            onDone(songs.size - failed, failed)
        }.start()
    }
}
