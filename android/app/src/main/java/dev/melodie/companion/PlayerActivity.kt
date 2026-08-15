package dev.melodie.companion

import android.app.Activity
import android.content.ComponentName
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.TextView
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.ListenableFuture

/** Handoff from the library screen; cleared as soon as it is consumed. */
object Queue {
    var pending: List<LocalSong> = emptyList()
}

class PlayerActivity : Activity() {

    companion object {
        const val EXTRA_INDEX = "index"
    }

    private val main = Handler(Looper.getMainLooper())
    private var controller: MediaController? = null
    private var controllerFuture: ListenableFuture<MediaController>? = null
    private var startIndex = 0
    private lateinit var nowPlaying: TextView
    private lateinit var position: TextView
    private lateinit var seek: SeekBar
    private lateinit var playPause: android.widget.Button
    private var userIsSeeking = false

    private val tick = object : Runnable {
        override fun run() {
            val c = controller ?: return
            if (!userIsSeeking && c.duration > 0) {
                seek.max = (c.duration / 1000).toInt()
                seek.progress = (c.currentPosition / 1000).toInt()
                position.text = "${clock(c.currentPosition)} / ${clock(c.duration)}"
            }
            playPause.text = if (c.isPlaying) "Pause" else "Play"
            nowPlaying.text = c.mediaMetadata.title?.toString() ?: "Nothing playing"
            // Only ticks while this screen is visible — stopped in onStop.
            main.postDelayed(this, 1000)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val root: LinearLayout = Ui.column(this)
        nowPlaying = Ui.label(this, "Connecting…", 20f)
        position = Ui.dim(this, "0:00 / 0:00")
        seek = SeekBar(this).apply {
            layoutParams = LinearLayout.LayoutParams(
                Ui.dp(this@PlayerActivity, 300),
                LinearLayout.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = Ui.dp(this@PlayerActivity, 8) }
        }
        playPause = Ui.button(this, "Play") { toggle() }
        root.addView(nowPlaying)
        root.addView(position)
        root.addView(seek)
        root.addView(playPause)
        root.addView(Ui.button(this, "Previous") { controller?.seekToPreviousMediaItem() })
        root.addView(Ui.button(this, "Next") { controller?.seekToNextMediaItem() })
        setContentView(root)

        seek.setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
            override fun onProgressChanged(bar: SeekBar?, value: Int, fromUser: Boolean) {}
            override fun onStartTrackingTouch(bar: SeekBar?) { userIsSeeking = true }
            override fun onStopTrackingTouch(bar: SeekBar?) {
                userIsSeeking = false
                controller?.seekTo(seek.progress * 1000L)
            }
        })

        startIndex = intent.getIntExtra(EXTRA_INDEX, 0)
    }

    override fun onStart() {
        super.onStart()
        if (controller == null) connect(startIndex)
    }

    private fun connect(startIndex: Int) {
        val token = SessionToken(this, ComponentName(this, PlaybackService::class.java))
        val future = MediaController.Builder(this, token).buildAsync()
        controllerFuture = future
        future.addListener({
            if (controllerFuture !== future) {
                // A newer connect() (or onStop's cleanup) has already
                // superseded this attempt — back out without touching any
                // state. Otherwise a future that completes right as onStop
                // synchronously releases it via the "already done" branch
                // of releaseFuture() would still stash the now-dead
                // controller here and restart the position tick with
                // nothing left to ever stop it.
                return@addListener
            }
            val c = try {
                future.get()
            } catch (e: java.util.concurrent.CancellationException) {
                return@addListener
            } catch (e: java.util.concurrent.ExecutionException) {
                return@addListener
            }
            controller = c
            val queue = Queue.pending
            if (queue.isNotEmpty()) {
                Queue.pending = emptyList()
                c.setMediaItems(queue.map { toMediaItem(it) }, startIndex, 0L)
                c.prepare()
                c.play()
            }
            main.post(tick)
        }, java.util.concurrent.Executor { main.post(it) })
    }

    private fun toMediaItem(song: LocalSong): MediaItem = MediaItem.Builder()
        .setUri(Uri.fromFile(LocalLibrary.fileFor(filesDir, song)))
        .setMediaId(song.id)
        .setMediaMetadata(
            MediaMetadata.Builder()
                .setTitle(song.title)
                .setArtist(song.artist)
                .setAlbumTitle(song.album)
                .build()
        )
        .build()

    private fun toggle() {
        val c = controller ?: return
        if (c.isPlaying) c.pause() else c.play()
    }

    private fun clock(ms: Long): String {
        val total = (ms / 1000).coerceAtLeast(0)
        return "${total / 60}:${(total % 60).toString().padStart(2, '0')}"
    }

    override fun onStop() {
        super.onStop()
        main.removeCallbacks(tick)
        controllerFuture?.let { MediaController.releaseFuture(it) }
        controller?.release()
        controller = null
        controllerFuture = null
    }

    override fun onDestroy() {
        main.removeCallbacks(tick)
        controller?.release()
        controller = null
        super.onDestroy()
    }
}
