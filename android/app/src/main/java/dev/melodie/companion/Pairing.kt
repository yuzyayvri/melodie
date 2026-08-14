package dev.melodie.companion

import android.content.Context

/** Where the desktop lives, and the secret that gets in. No accounts. */
data class Server(val host: String, val port: Int, val token: String)

/**
 * Parses the payload the desktop's QR code carries:
 * `melodie://<host>:<port>/<token>`.
 *
 * Hand-parsed rather than via `Uri.parse` so it can be unit-tested off-device
 * — `android.net.Uri` is a stub in JVM tests.
 */
fun parsePairingUrl(raw: String): Server? {
    val rest = raw.trim().removePrefix("melodie://")
    if (rest == raw.trim()) return null
    val slash = rest.indexOf('/')
    if (slash <= 0) return null
    val authority = rest.substring(0, slash)
    val token = rest.substring(slash + 1).trim()
    if (token.isEmpty()) return null
    val colon = authority.lastIndexOf(':')
    if (colon <= 0) return null
    val port = authority.substring(colon + 1).toIntOrNull() ?: return null
    if (port !in 1..65535) return null
    val host = authority.substring(0, colon)
    if (host.isEmpty()) return null
    return Server(host, port, token)
}

/** The paired server, remembered across launches. */
object Pairing {
    private const val PREFS = "pairing"

    fun load(context: Context): Server? {
        val p = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val host = p.getString("host", null) ?: return null
        val token = p.getString("token", null) ?: return null
        return Server(host, p.getInt("port", 4533), token)
    }

    fun save(context: Context, server: Server) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString("host", server.host)
            .putInt("port", server.port)
            .putString("token", server.token)
            .apply()
    }

    fun clear(context: Context) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().clear().apply()
    }
}
