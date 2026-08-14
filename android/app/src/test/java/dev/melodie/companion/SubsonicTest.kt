package dev.melodie.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.kxml2.io.KXmlParser
import org.xmlpull.v1.XmlPullParser
import java.io.StringReader

class SubsonicTest {

    private fun parserFor(xml: String): XmlPullParser =
        KXmlParser().apply {
            setFeature(XmlPullParser.FEATURE_PROCESS_NAMESPACES, false)
            setInput(StringReader(xml))
        }

    @Test
    fun md5MatchesTheServersSaltedTokenScheme() {
        // Known vector: md5("abc") — the desktop computes md5(token + salt)
        // with exactly this function, so a mismatch here means no request
        // ever authenticates.
        assertEquals("900150983cd24fb0d6963f7d28e17f72", Subsonic.md5("abc"))
        assertEquals("d41d8cd98f00b204e9800998ecf8427e", Subsonic.md5(""))
    }

    @Test
    fun authQueryCarriesTokenAndSaltNotThePassword() {
        val q = Subsonic.authQuery("s3cret", "abc")
        assertTrue(q, q.contains("s=abc"))
        assertTrue(q, q.contains("t=" + Subsonic.md5("s3cretabc")))
        assertTrue(q, q.contains("v=1.16.1"))
        assertTrue(q, q.contains("c=melodie"))
        assertTrue("the shared secret must never travel in clear", !q.contains("s3cret&") && !q.contains("p="))
    }

    @Test
    fun urlBuildsAgainstTheServerAddress() {
        val server = Server("192.168.1.20", 4533, "tok")
        val url = Subsonic.url(server, "getPlaylists", "")
        assertTrue(url, url.startsWith("http://192.168.1.20:4533/rest/getPlaylists.view?"))
    }

    @Test
    fun parsePairingUrlAcceptsTheDesktopsFormat() {
        val s = parsePairingUrl("melodie://192.168.1.20:4533/abcTOKEN123")
        assertEquals("192.168.1.20", s!!.host)
        assertEquals(4533, s.port)
        assertEquals("abcTOKEN123", s.token)

        assertNull(parsePairingUrl("http://192.168.1.20:4533/tok"))
        assertNull(parsePairingUrl("melodie://192.168.1.20/tok"))
        assertNull(parsePairingUrl("melodie://192.168.1.20:notaport/tok"))
        assertNull(parsePairingUrl("nonsense"))
        assertNull(parsePairingUrl("melodie://192.168.1.20:4533/"))
    }

    @Test
    fun parsesPlaylistListing() {
        val xml = """
            <?xml version="1.0" encoding="UTF-8"?>
            <subsonic-response status="ok" version="1.16.1">
              <playlists>
                <playlist id="pl1" name="Chill" songCount="2" duration="420"/>
                <playlist id="pl2" name="Rock &amp; Roll" songCount="17" duration="3600"/>
              </playlists>
            </subsonic-response>
        """.trimIndent()
        val out = Subsonic.parseAll(parserFor(xml), "playlist") {
            RemotePlaylist(
                it.getAttributeValue(null, "id"),
                it.getAttributeValue(null, "name"),
                it.getAttributeValue(null, "songCount")?.toIntOrNull() ?: 0,
            )
        }
        assertEquals(2, out.size)
        assertEquals("Chill", out[0].name)
        assertEquals(2, out[0].songCount)
        assertEquals("Rock & Roll", out[1].name, )
    }

    @Test
    fun parsesPlaylistEntries() {
        val xml = """
            <subsonic-response status="ok" version="1.16.1">
              <playlist id="pl1" name="Chill" songCount="1">
                <entry id="tr7" title="Rhubarb" artist="Aphex Twin" album="SAW II"
                       duration="210" suffix="mp3" contentType="audio/mpeg"/>
              </playlist>
            </subsonic-response>
        """.trimIndent()
        val out = Subsonic.parseAll(parserFor(xml), "entry") {
            RemoteSong(
                it.getAttributeValue(null, "id"),
                it.getAttributeValue(null, "title"),
                it.getAttributeValue(null, "artist") ?: "",
                it.getAttributeValue(null, "album") ?: "",
                it.getAttributeValue(null, "duration")?.toIntOrNull() ?: 0,
                it.getAttributeValue(null, "suffix") ?: "mp3",
            )
        }
        assertEquals(1, out.size)
        assertEquals("tr7", out[0].id)
        assertEquals(210, out[0].durationSec)
        assertEquals("mp3", out[0].suffix)
    }

    @Test
    fun serverErrorBecomesAnException() {
        val xml = """
            <subsonic-response status="failed" version="1.16.1">
              <error code="40" message="Wrong username or password"/>
            </subsonic-response>
        """.trimIndent()
        try {
            Subsonic.parseAll(parserFor(xml), "playlist") { }
            throw AssertionError("expected a SubsonicException")
        } catch (e: SubsonicException) {
            assertEquals(40, e.code)
            assertTrue(e.message!!, e.message!!.contains("Wrong username"))
        }
    }
}
