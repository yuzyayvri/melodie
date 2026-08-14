package dev.melodie.companion

import android.app.Activity
import android.os.Bundle
import android.widget.TextView

class PairActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = "Melodie" })
    }
}
