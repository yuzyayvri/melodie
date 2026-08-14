package dev.melodie.companion

import android.app.Activity
import android.os.Bundle
import android.widget.TextView

// Placeholder — Task 5 replaces this with the real library screen.
class LibraryActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply { text = "Melodie" })
    }
}
