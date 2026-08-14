package dev.melodie.companion

import android.content.Context
import android.graphics.Color
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView

/** Views in code, not XML: no layout inflation, no AppCompat, no resources. */
object Ui {
    val BG = Color.parseColor("#1B1B1F")
    val BG_ALT = Color.parseColor("#26262B")
    val FG = Color.parseColor("#E6E6EA")
    val FG_DIM = Color.parseColor("#9A9AA4")
    val ACCENT = Color.parseColor("#E8B44A")

    fun dp(context: Context, value: Int): Int =
        (value * context.resources.displayMetrics.density).toInt()

    fun column(context: Context): LinearLayout = LinearLayout(context).apply {
        orientation = LinearLayout.VERTICAL
        setBackgroundColor(BG)
        val p = dp(context, 16)
        setPadding(p, p, p, p)
        layoutParams = ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
    }

    fun label(context: Context, text: String, size: Float = 16f): TextView =
        TextView(context).apply {
            this.text = text
            textSize = size
            setTextColor(FG)
            setPadding(0, dp(context, 4), 0, dp(context, 4))
        }

    fun dim(context: Context, text: String, size: Float = 13f): TextView =
        label(context, text, size).apply { setTextColor(FG_DIM) }

    fun input(context: Context, hint: String): EditText = EditText(context).apply {
        this.hint = hint
        setTextColor(FG)
        setHintTextColor(FG_DIM)
    }

    fun button(context: Context, text: String, onClick: () -> Unit): Button =
        Button(context).apply {
            this.text = text
            setTextColor(BG)
            setBackgroundColor(ACCENT)
            setOnClickListener { onClick() }
        }
}
