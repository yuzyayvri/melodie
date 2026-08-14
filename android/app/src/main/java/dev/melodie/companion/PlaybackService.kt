package dev.melodie.companion

import android.app.PendingIntent
import android.content.Intent
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService

/**
 * PLAN.md §1/§7: reliable background audio on Android means a
 * MediaSessionService holding the player. This is what keeps playback alive
 * with the app off-screen and gives lockscreen, headset and Bluetooth
 * controls for free.
 */
class PlaybackService : MediaSessionService() {

    private var session: MediaSession? = null

    override fun onCreate() {
        super.onCreate()
        val player = ExoPlayer.Builder(this).build()
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, PlayerActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        session = MediaSession.Builder(this, player)
            .setSessionActivity(open)
            .build()
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

    override fun onTaskRemoved(rootIntent: Intent?) {
        // Swiping the app away while paused should not leave a silent
        // service running; while playing, playback continues on purpose.
        val player = session?.player
        if (player == null || !player.isPlaying) {
            stopSelf()
        }
    }

    override fun onDestroy() {
        session?.run {
            player.release()
            release()
        }
        session = null
        super.onDestroy()
    }
}
