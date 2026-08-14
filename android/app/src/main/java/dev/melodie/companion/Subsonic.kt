package dev.melodie.companion

import android.util.Xml
import org.xmlpull.v1.XmlPullParser
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.security.MessageDigest

data class RemotePlaylist(val id: String, val name: String, val songCount: Int)

data class RemoteSong(
    val id: String,
    val title: String,
    val artist: String,
    val album: String,
    val durationSec: Int,
    val suffix: String,
)

class SubsonicException(val code: Int, message: String) : Exception(message)

object Subsonic {
    const val API_VERSION = "1.16.1"
    private const val CLIENT = "melodie"

    fun md5(input: String): String {
        val digest = MessageDigest.getInstance("MD5").digest(input.toByteArray(Charsets.UTF_8))
        val out = StringBuilder(digest.size * 2)
        for (b in digest) {
            val v = b.toInt() and 0xff
            out.append("0123456789abcdef"[v ushr 4])
            out.append("0123456789abcdef"[v and 0x0f])
        }
        return out.toString()
    }

    /**
     * Subsonic's salted-token auth: the shared secret itself never travels,
     * only md5(secret + salt) with a fresh salt per request.
     */
    fun authQuery(token: String, salt: String): String =
        "u=melodie&t=${md5(token + salt)}&s=$salt&v=$API_VERSION&c=$CLIENT"

    private fun freshSalt(): String {
        val alphabet = "abcdefghijklmnopqrstuvwxyz0123456789"
        val rnd = java.security.SecureRandom()
        return (1..12).map { alphabet[rnd.nextInt(alphabet.length)] }.joinToString("")
    }

    fun url(server: Server, method: String, params: String): String {
        val auth = authQuery(server.token, freshSalt())
        val extra = if (params.isEmpty()) "" else "&$params"
        return "http://${server.host}:${server.port}/rest/$method.view?$auth$extra"
    }

    fun encode(value: String): String = URLEncoder.encode(value, "UTF-8")

    /**
     * Streams through the response pulling out every `<tag>` element.
     *
     * Deliberately a pull parser, not DOM: a playlist response for a large
     * library would otherwise be materialised whole in RAM on the phone.
     */
    fun <T> parseAll(parser: XmlPullParser, tag: String, map: (XmlPullParser) -> T): List<T> {
        val out = ArrayList<T>()
        var event = parser.eventType
        while (event != XmlPullParser.END_DOCUMENT) {
            if (event == XmlPullParser.START_TAG) {
                when (parser.name) {
                    "error" -> throw SubsonicException(
                        parser.getAttributeValue(null, "code")?.toIntOrNull() ?: 0,
                        parser.getAttributeValue(null, "message") ?: "Unknown server error",
                    )
                    tag -> out.add(map(parser))
                }
            }
            event = parser.next()
        }
        return out
    }

    fun newParser(input: InputStream): XmlPullParser = Xml.newPullParser().apply {
        setFeature(XmlPullParser.FEATURE_PROCESS_NAMESPACES, false)
        setInput(input, null)
    }
}

/**
 * Every call is a blocking GET — callers run it on a background thread.
 * There is no connection pooling, no retry policy and no cache: this talks
 * to one server on the same Wi-Fi, a handful of times per sync.
 */
class SubsonicClient(val server: Server) {

    private fun open(method: String, params: String = ""): HttpURLConnection {
        val connection = URL(Subsonic.url(server, method, params)).openConnection() as HttpURLConnection
        connection.connectTimeout = 5_000
        connection.readTimeout = 20_000
        connection.requestMethod = "GET"
        return connection
    }

    private fun <T> get(method: String, params: String, tag: String, map: (XmlPullParser) -> T): List<T> {
        val connection = open(method, params)
        try {
            connection.inputStream.use { return Subsonic.parseAll(Subsonic.newParser(it), tag, map) }
        } finally {
            connection.disconnect()
        }
    }

    fun ping(): Boolean = try {
        get("ping", "", "nothing") { }
        true
    } catch (e: Exception) {
        false
    }

    fun playlists(): List<RemotePlaylist> = get("getPlaylists", "", "playlist") {
        RemotePlaylist(
            it.getAttributeValue(null, "id") ?: "",
            it.getAttributeValue(null, "name") ?: "(unnamed)",
            it.getAttributeValue(null, "songCount")?.toIntOrNull() ?: 0,
        )
    }

    fun playlist(id: String): List<RemoteSong> =
        get("getPlaylist", "id=${Subsonic.encode(id)}", "entry") { toSong(it) }

    private fun toSong(p: XmlPullParser) = RemoteSong(
        p.getAttributeValue(null, "id") ?: "",
        p.getAttributeValue(null, "title") ?: "(untitled)",
        p.getAttributeValue(null, "artist") ?: "",
        p.getAttributeValue(null, "album") ?: "",
        p.getAttributeValue(null, "duration")?.toIntOrNull() ?: 0,
        p.getAttributeValue(null, "suffix") ?: "mp3",
    )

    /**
     * Downloads to a temp file and renames on success, so an interrupted
     * sync can never leave a half-file that looks synced.
     *
     * Copies through a fixed 64 KB buffer — the file never exists in memory.
     */
    fun downloadTo(songId: String, target: File, onBytes: (Long) -> Unit) {
        val connection = open("download", "id=${Subsonic.encode(songId)}")
        val partial = File(target.parentFile, target.name + ".part")
        try {
            if (connection.responseCode != 200) {
                throw SubsonicException(connection.responseCode, "download failed for $songId")
            }
            // tiny_http's error path (src/server.rs on the desktop) answers
            // a bad id or missing file with HTTP 200 and an XML error
            // envelope, not a non-200 status — catch that here so it never
            // gets written to disk and mistaken for real audio.
            if (connection.contentType?.startsWith("text/xml") == true) {
                throw SubsonicException(0, "server returned an error for $songId")
            }
            connection.inputStream.use { input ->
                FileOutputStream(partial).use { output ->
                    val buffer = ByteArray(64 * 1024)
                    var total = 0L
                    while (true) {
                        val read = input.read(buffer)
                        if (read < 0) break
                        output.write(buffer, 0, read)
                        total += read
                        onBytes(total)
                    }
                }
            }
            if (!partial.renameTo(target)) throw SubsonicException(0, "could not finalise ${target.name}")
        } catch (e: Exception) {
            partial.delete()
            throw e
        } finally {
            connection.disconnect()
        }
    }
}
