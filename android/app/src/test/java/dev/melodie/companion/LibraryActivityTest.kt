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

    @Test
    fun sortFieldOrderMatchesSpinnerLabels() {
        assertEquals(listOf("TITLE", "ARTIST", "ALBUM", "DURATION"), SortField.entries.map { it.name })
    }
}
