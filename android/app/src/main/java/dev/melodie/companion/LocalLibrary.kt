package dev.melodie.companion

import java.io.File

/** A track that exists on this phone, and which playlist put it here. */
data class LocalSong(
    val id: String,
    val title: String,
    val artist: String,
    val album: String,
    val durationSec: Int,
    val fileName: String,
    val playlist: String,
)

/**
 * The phone's record of what it has synced: one tab-separated line per track
 * in `library.tsv`.
 *
 * Not JSON, not a database. `org.json` is an Android stub that cannot be
 * unit-tested off-device, Room is a dependency and a compiler plugin for
 * something with no queries, and this file is a few hundred lines of text.
 */
object LocalLibrary {
    private const val MANIFEST = "library.tsv"

    /** Tabs and newlines are the field and record separators, so they cannot survive in a value. */
    private fun clean(value: String): String =
        value.replace('\t', ' ').replace('\n', ' ').replace('\r', ' ')

    fun encodeLine(s: LocalSong): String = listOf(
        clean(s.id),
        clean(s.title),
        clean(s.artist),
        clean(s.album),
        s.durationSec.toString(),
        clean(s.fileName),
        clean(s.playlist),
    ).joinToString("\t")

    fun decodeLine(line: String): LocalSong? {
        if (line.isBlank() || line.startsWith("#")) return null
        val f = line.split('\t')
        if (f.size != 7) return null
        return LocalSong(
            id = f[0],
            title = f[1],
            artist = f[2],
            album = f[3],
            durationSec = f[4].toIntOrNull() ?: 0,
            fileName = f[5],
            playlist = f[6],
        )
    }

    private fun manifestFile(dir: File) = File(dir, MANIFEST)

    fun tracksDir(dir: File): File = File(dir, "tracks").apply { mkdirs() }

    fun fileFor(dir: File, song: LocalSong): File = File(tracksDir(dir), song.fileName)

    fun load(dir: File): List<LocalSong> {
        val file = manifestFile(dir)
        if (!file.exists()) return emptyList()
        return file.readLines().mapNotNull { decodeLine(it) }
    }

    fun save(dir: File, songs: List<LocalSong>) {
        dir.mkdirs()
        // Write-then-rename: a killed process can't leave a truncated manifest
        // that would make synced tracks look missing.
        val tmp = File(dir, "$MANIFEST.tmp")
        tmp.writeText(songs.joinToString("\n") { encodeLine(it) })
        tmp.renameTo(manifestFile(dir))
    }
}
