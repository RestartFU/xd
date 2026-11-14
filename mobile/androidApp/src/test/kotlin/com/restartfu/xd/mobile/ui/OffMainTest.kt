package com.restartfu.xd.mobile.ui

import android.os.Looper
import androidx.compose.ui.test.junit4.createComposeRule
import java.util.concurrent.CopyOnWriteArrayList
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * A reply already streaming when its chat opens is read away from the main
 * thread, its first reading included: the first frame draws nothing parsed,
 * and the caller shows the plain text until the reading is ready.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class OffMainTest {
    @get:Rule
    val compose = createComposeRule()

    @Test fun theFirstReadingIsDoneOffTheMainThreadToo() {
        val onMain = CopyOnWriteArrayList<Boolean>()
        val seen = CopyOnWriteArrayList<String?>()
        val key = "a long reply"
        compose.setContent {
            val read = rememberOffMain(key) { next: String, _: String? ->
                onMain += Looper.myLooper() == Looper.getMainLooper()
                next.uppercase()
            }
            seen += read
        }
        // The first composition has nothing parsed to draw.
        assertNull(seen.first())
        compose.waitUntil(5_000) { seen.lastOrNull() == "A LONG REPLY" }
        // Worked out once, and not on the main thread.
        assertEquals(listOf(false), onMain.toList())
    }
}
