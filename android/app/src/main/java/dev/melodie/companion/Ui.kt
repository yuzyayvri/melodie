package dev.melodie.companion

import android.content.Context
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.text.TextUtils
import android.view.Gravity
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Views in code, not XML: no layout inflation, no AppCompat, no resources.
 * Palette and rounded-flat button look mirror the desktop's `ui/theme.rs`
 * (same hex values) so the phone doesn't feel like a different app.
 */
object Ui {
    val BG = Color.parseColor("#1A1B1E")
    val BG_ALT = Color.parseColor("#22232B")
    val FG = Color.parseColor("#ECECEE")
    val FG_DIM = Color.parseColor("#8D8D94")
    val ACCENT = Color.parseColor("#6CA8FF")

    /** Caps single-column content at a readable width on tall/wide phones instead of edge-to-edge. */
    private const val MAX_CONTENT_DP = 300

    fun dp(context: Context, value: Int): Int =
        (value * context.resources.displayMetrics.density).toInt()

    fun column(context: Context): LinearLayout = LinearLayout(context).apply {
        orientation = LinearLayout.VERTICAL
        gravity = Gravity.CENTER_HORIZONTAL
        setBackgroundColor(BG)
        val p = dp(context, 16)
        setPadding(p, p, p, p)
        layoutParams = ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
        // targetSdk 36 enforces edge-to-edge, so content draws under the
        // status bar / camera cutout / nav bar unless padded for it. Fixed
        // once here (every screen's root is `column()`) rather than per
        // activity. `systemWindowInsetTop/Bottom` (deprecated in API 30
        // favor of `WindowInsets.Type`) is used on purpose: it's been
        // functional since API 20, so there's no SDK_INT branch needed
        // against this app's minSdk 24.
        setOnApplyWindowInsetsListener { v, insets ->
            @Suppress("DEPRECATION")
            v.setPadding(p, p + insets.systemWindowInsetTop, p, p + insets.systemWindowInsetBottom)
            insets
        }
    }

    /**
     * A back chevron (only when `onBack` is non-null) plus a bold title —
     * the hand-rolled stand-in for a `Toolbar`/`ActionBar`, which would
     * pull in `androidx.activity`/AppCompat. Every non-root screen gets
     * one so there's always an on-screen way back, not just gesture/system
     * back.
     */
    fun topBar(context: Context, title: String, onBack: (() -> Unit)?): LinearLayout =
        LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
            if (onBack != null) {
                addView(TextView(context).apply {
                    text = "‹" // ‹
                    textSize = 22f
                    setTextColor(FG)
                    gravity = Gravity.CENTER
                    minWidth = dp(context, 48)
                    minHeight = dp(context, 48)
                    isClickable = true
                    isFocusable = true
                    setOnClickListener { onBack() }
                })
            }
            addView(TextView(context).apply {
                text = title
                textSize = 18f
                setTypeface(typeface, Typeface.BOLD)
                setTextColor(FG)
                maxLines = 1
                ellipsize = TextUtils.TruncateAt.END
                val leftPad = if (onBack != null) 0 else dp(context, 12)
                setPadding(leftPad, dp(context, 8), dp(context, 12), dp(context, 8))
                layoutParams = LinearLayout.LayoutParams(
                    0,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                    1f,
                )
            })
        }

    fun label(context: Context, text: String, size: Float = 16f): TextView =
        TextView(context).apply {
            this.text = text
            textSize = size
            setTextColor(FG)
            gravity = Gravity.CENTER
            maxWidth = dp(context, MAX_CONTENT_DP)
            setPadding(0, dp(context, 6), 0, dp(context, 6))
        }

    fun dim(context: Context, text: String, size: Float = 13f): TextView =
        label(context, text, size).apply { setTextColor(FG_DIM) }

    fun input(context: Context, hint: String): EditText = EditText(context).apply {
        this.hint = hint
        setTextColor(FG)
        setHintTextColor(FG_DIM)
        gravity = Gravity.CENTER
        background = rounded(fill = BG_ALT, stroke = FG_DIM, radiusDp = 8, context = context)
        val h = dp(context, 12)
        val v = dp(context, 10)
        setPadding(h, v, h, v)
        layoutParams = LinearLayout.LayoutParams(dp(context, MAX_CONTENT_DP), ViewGroup.LayoutParams.WRAP_CONTENT).apply {
            topMargin = dp(context, 6)
        }
    }

    fun button(context: Context, text: String, onClick: () -> Unit): Button =
        Button(context).apply {
            this.text = text
            isAllCaps = false
            setTextColor(BG)
            background = rounded(fill = ACCENT, stroke = null, radiusDp = 8, context = context)
            val h = dp(context, 20)
            val v = dp(context, 12)
            setPadding(h, v, h, v)
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = dp(context, 10) }
            setOnClickListener { onClick() }
        }

    private fun rounded(fill: Int, stroke: Int?, radiusDp: Int, context: Context): GradientDrawable =
        GradientDrawable().apply {
            setColor(fill)
            cornerRadius = dp(context, radiusDp).toFloat()
            stroke?.let { setStroke(dp(context, 1), it) }
        }
}
