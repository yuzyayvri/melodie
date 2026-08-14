package dev.melodie.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class LocalLibraryTest {

    @get:Rule
    val temp = TemporaryFolder()

    private fun song(id: String, title: String, playlist: String = "Chill") = LocalSong(
        id = id,
        title = title,
        artist = "Aphex Twin",
        album = "SAW II",
        durationSec = 210,
        fileName = "$id.mp3",
        playlist = playlist,
    )

    @Test
    fun linesRoundTrip() {
        val s = song("tr7", "Rhubarb")
        assertEquals(s, LocalLibrary.decodeLine(LocalLibrary.encodeLine(s)))
    }

    @Test
    fun tabsAndNewlinesInMetadataCannotCorruptTheFile() {
        val s = song("tr8", "Weird\tTitle\nSecond line")
        val decoded = LocalLibrary.decodeLine(LocalLibrary.encodeLine(s))!!
        assertEquals("tr8", decoded.id)
        assertEquals(1, LocalLibrary.encodeLine(s).lines().size)
        assertEquals("Weird Title Second line", decoded.title)
    }

    @Test
    fun malformedLinesAreSkippedNotFatal() {
        assertNull(LocalLibrary.decodeLine(""))
        assertNull(LocalLibrary.decodeLine("only\ttwo"))
        assertNull(LocalLibrary.decodeLine("#comment"))
    }

    @Test
    fun saveThenLoadPreservesEverything() {
        val dir = temp.newFolder()
        val songs = listOf(song("tr1", "Rhubarb"), song("tr2", "Curtains", playlist = "Focus"))
        LocalLibrary.save(dir, songs)
        assertEquals(songs, LocalLibrary.load(dir))
    }

    @Test
    fun loadOnAFreshInstallIsEmptyNotAnError() {
        assertEquals(emptyList<LocalSong>(), LocalLibrary.load(temp.newFolder()))
    }
}
