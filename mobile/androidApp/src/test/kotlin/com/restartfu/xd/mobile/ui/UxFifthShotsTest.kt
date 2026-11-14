package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.performClick
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import java.io.File
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Captures the surfaces the fifth audit's fixes changed, from the whole app
 * against [AuditHost]. With `-Pxd.shots=<dir>` it writes
 * `android-<surface>-<theme>.png` there.
 *
 * It asserts nothing about what the app says, and uses only what the app had
 * before those fixes, so the same file run on the commit before them gives
 * the matching "before" captures. A pairing code that cannot be read is the
 * notice in each: the app has said that on its snackbar all along.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxFifthShotsTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** A notice while the keyboard is up in a chat. */
    @Test fun snackbarWithTheKeyboard() = shoot("android-snackbar-keyboard-dark", MinimalThemePreset.DARK, keyboard = true) {
        openChat()
        showKeyboard(compose.activity, keyboardPx(), navigationPx())
        idle()
        notice()
    }

    /** A notice in a chat, over its composer. */
    @Test fun snackbarInAChat() = shoot("android-snackbar-chat-dark", MinimalThemePreset.DARK) {
        openChat()
        notice()
    }

    @Test fun snackbarInAChatLight() = shoot("android-snackbar-chat-light", MinimalThemePreset.LIGHT) {
        openChat()
        notice()
    }

    /** The settling stepper, in Settings › Cleanup. */
    @Test fun settleStepperDark() = shoot("android-settle-stepper-dark", MinimalThemePreset.DARK) { cleanup() }

    @Test fun settleStepperLight() = shoot("android-settle-stepper-light", MinimalThemePreset.LIGHT) { cleanup() }

    private lateinit var host: AuditHost
    private lateinit var model: MainViewModel
    private var keyboardShown = false

    private fun keyboardPx() = (KEYBOARD_DP * compose.activity.resources.displayMetrics.density).toInt()

    private fun navigationPx() = (NAVIGATION_DP * compose.activity.resources.displayMetrics.density).toInt()

    private fun ProjectsScreenshotTest.Screen.openChat() {
        click("Reconnect backoff")
        await { shows("3 tool calls") }
        settle()
    }

    private fun ProjectsScreenshotTest.Screen.notice() {
        compose.runOnUiThread { model.connectPairing("", "") }
        ShadowLooper.idleMainLooper()
        repeat(2) { settle() }
    }

    private fun ProjectsScreenshotTest.Screen.cleanup() {
        click("Settings")
        await { shows("Search settings") }
        compose.onAllNodes(hasText("Cleanup")).onFirst().performClick()
        idle()
        await { shows("Days before settling") }
        idle()
    }

    private fun ProjectsScreenshotTest.Screen.idle() {
        ShadowLooper.idleMainLooper()
        settle()
    }

    private fun ProjectsScreenshotTest.Screen.await(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 20_000
        while (!condition() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            settle()
        }
        check(condition()) { "Timed out; showing ${labels()}" }
    }

    private fun shoot(
        name: String,
        theme: MinimalThemePreset,
        keyboard: Boolean = false,
        steps: ProjectsScreenshotTest.Screen.() -> Unit = {},
    ) {
        val activity = compose.activity
        // Animations off, as "Remove animations" sets it, so a capture does not catch a page mid-move.
        Settings.Global.putFloat(activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        keyboardShown = keyboard
        host = AuditHost().connect()
        model = MainViewModel(activity.application, host.client)
        val settings = MobileSettings(activity)
        settings.setTheme(theme)
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val live by model.client.link.collectAsStateWithLifecycle()
            XdRoot(theme) { MinimalMobileApp(model, live, settings) }
        }
        val screen = ProjectsScreenshotTest.Screen(compose, activity)
        try {
            screen.await { model.client.tree.value.folders.isNotEmpty() }
            screen.settle()
            screen.steps()
        } catch (failure: Throwable) {
            capture(screen, "$name-failed")
            throw failure
        }
        capture(screen, name)
        host.close()
    }

    private fun capture(screen: ProjectsScreenshotTest.Screen, name: String) {
        val dir = out ?: return
        dir.mkdirs()
        val bitmap = screen.capture()
        if (keyboardShown) {
            // Where the keyboard and the navigation bar beneath it sit.
            val top = bitmap.height - keyboardPx() - navigationPx()
            Canvas(bitmap).drawRect(
                0f, top.toFloat(), bitmap.width.toFloat(), bitmap.height.toFloat(),
                Paint().apply { color = android.graphics.Color.argb(235, 96, 98, 104) },
            )
        }
        File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
    }

    private companion object {
        const val KEYBOARD_DP = 300
        const val NAVIGATION_DP = 48
    }
}
