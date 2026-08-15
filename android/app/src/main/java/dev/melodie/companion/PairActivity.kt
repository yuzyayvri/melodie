package dev.melodie.companion

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.Toast
import com.journeyapps.barcodescanner.ScanIntentResult
import com.journeyapps.barcodescanner.ScanOptions

/**
 * Pairing (PLAN.md §7): scan the QR the desktop shows, or type the same
 * details by hand. No account, no login.
 *
 * Scanning uses the classic `startActivityForResult` API rather than
 * ZXing's `ScanContract`/`registerForActivityResult`: `ScanContract`
 * extends `androidx.activity.result.contract.ActivityResultContract`, and
 * `registerForActivityResult` itself only exists on `ComponentActivity`.
 * Neither is available without adding `androidx.activity` as a dependency,
 * which this project's fixed three-dependency budget (media3-exoplayer,
 * media3-session, zxing-android-embedded) doesn't allow. `ScanOptions`
 * still builds a plain scan `Intent` via `createScanIntent`, and
 * `ScanIntentResult.parseActivityResult` reads the result back — both
 * confirmed present in the 4.3.0 artifact.
 */
class PairActivity : Activity() {

    private val main = Handler(Looper.getMainLooper())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Already paired: go straight to the library.
        if (Pairing.load(this) != null) {
            startActivity(Intent(this, LibraryActivity::class.java))
            finish()
            return
        }

        val root = Ui.column(this)
        root.addView(Ui.label(this, "Pair with Melodie", 22f))
        root.addView(
            Ui.dim(
                this,
                "On the desktop, click Pair (or run `melodie --pair`) and scan the code.",
            )
        )
        root.addView(Ui.button(this, "Scan QR code") {
            val options = ScanOptions()
                .setPrompt("Point at the code on your computer")
                .setBeepEnabled(false)
            startActivityForResult(options.createScanIntent(this), SCAN_REQUEST)
        })

        root.addView(Ui.dim(this, "\nOr enter it manually:"))
        val host = Ui.input(this, "Host, e.g. 192.168.1.20")
        val port = Ui.input(this, "Port, e.g. 4533")
        val token = Ui.input(this, "Token")
        root.addView(host)
        root.addView(port)
        root.addView(token)
        root.addView(Ui.button(this, "Connect") {
            val portValue = port.text.toString().trim().toIntOrNull()
            if (host.text.isBlank() || portValue == null || token.text.isBlank()) {
                toast("Fill in host, port and token")
            } else {
                verify(Server(host.text.toString().trim(), portValue, token.text.toString().trim()))
            }
        })

        setContentView(root)
    }

    @Deprecated("classic result API — avoids depending on androidx.activity")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != SCAN_REQUEST) return
        val contents = ScanIntentResult.parseActivityResult(resultCode, data).contents
        if (contents == null) {
            toast("Scan cancelled")
            return
        }
        val server = parsePairingUrl(contents)
        if (server == null) toast("That QR code isn't a Melodie pairing code") else verify(server)
    }

    /** Confirms the details actually work before storing them. */
    private fun verify(server: Server) {
        toast("Connecting…")
        Thread {
            val ok = SubsonicClient(server).ping()
            main.post {
                if (!ok) {
                    toast("Couldn't reach Melodie at ${server.host}:${server.port}")
                } else {
                    Pairing.save(this, server)
                    startActivity(Intent(this, LibraryActivity::class.java))
                    finish()
                }
            }
        }.start()
    }

    private fun toast(message: String) =
        Toast.makeText(this, message, Toast.LENGTH_SHORT).show()

    private companion object {
        // IntentIntegrator.REQUEST_CODE — the classic ZXing scan result code.
        const val SCAN_REQUEST = 49374
    }
}
