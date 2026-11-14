package com.restartfu.xd.mobile.ui

import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import java.io.File
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Captures the surfaces the third audit's fixes changed, from the whole app
 * against [AuditHost]. With `-Pxd.shots=<dir>` it writes
 * `android-<surface>-<theme>.png` there.
 *
 * It asserts nothing about what the app says, and uses only what the app had
 * before those fixes, so the same file run on the commit before them gives
 * the matching "before" captures.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxPolishShotsTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** Settings › Chats: its rows of switches. */
    @Test fun settingsChats() = shoot("android-settings-chats-dark", MinimalThemePreset.DARK) {
        click("Settings")
        await { shows("Chats") }
        click("Chats")
        repeat(3) { settle() }
    }

    /** The chat's own settings in Light: switch rows, choices, and hints once drawn in the outline colour. */
    @Test fun chatSettingsLight() = shoot("android-chat-settings-light", MinimalThemePreset.LIGHT) {
        openChat()
        click("More")
        click("Chat settings")
        repeat(3) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    /**
     * A file open for editing with the keyboard up. The keyboard is drawn as
     * a grey block where it sits, since Robolectric has none to draw.
     */
    @Test fun fileEditorWithTheKeyboard() = shoot("android-file-editor-ime-dark", MinimalThemePreset.DARK, keyboard = true) {
        openChat()
        click("Workspace view: Chat")
        click("Files")
        await { shows("README.md") }
        click("README.md")
        await { shows("Save") }
        showKeyboard(activity, keyboardPx(), navigationPx())
        repeat(4) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    /** The chat's terminal in Light: its key bar, and its header line once drawn in the outline colour. */
    @Test fun terminalKeysLight() = shoot("android-terminal-keys-light", MinimalThemePreset.LIGHT) {
        openChat()
        click("Workspace view: Chat")
        click("Terminal")
        await { asked("terminal-resize") }
        repeat(3) { settle() }
    }

    /** What a tap on the shared terminal's Stop does. */
    @Test fun terminalStop() = shoot("android-terminal-stop-dark", MinimalThemePreset.DARK) {
        click("Terminal")
        await { asked("terminal-resize") }
        repeat(3) { settle() }
        click("Stop")
        repeat(4) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    /** The Usage page's choices. */
    @Test fun usageChoices() = shoot("android-usage-choices-dark", MinimalThemePreset.DARK) {
        click("Usage")
        repeat(4) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    /** An approval whose patch runs to hundreds of lines. */
    @Test fun approvalWithALongDiff() = shoot("android-ask-long-diff-dark", MinimalThemePreset.DARK) {
        openChat()
        host.push(
            """{"event":"interaction","chat":"c1","request_id":"r-long","method":"approval","interaction_revision":1,""" +
                """"params":{"tool":"Edit","command":"Edit shared/src/commonMain/kotlin/com/restartfu/xd/net/Backoff.kt",""" +
                """"patch":${JsonPrimitive(LONG_PATCH)}}}""",
        )
        await { shows("Apply this edit?") }
        repeat(6) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    /** New session with the largest text Android offers. */
    @Test fun newSessionWithEnlargedText() =
        shoot("android-new-session-fontscale2-dark", MinimalThemePreset.DARK, fontScale = 2f) {
            click("New session in xd")
            repeat(6) {
                ShadowLooper.idleMainLooper()
                settle()
            }
        }

    private lateinit var host: AuditHost
    private var keyboardShown = false

    private fun keyboardPx() = (KEYBOARD_DP * compose.activity.resources.displayMetrics.density).toInt()

    private fun navigationPx() = (NAVIGATION_DP * compose.activity.resources.displayMetrics.density).toInt()

    private val ProjectsScreenshotTest.Screen.activity get() = compose.activity

    private fun ProjectsScreenshotTest.Screen.openChat() {
        click("Reconnect backoff")
        await { shows("3 tool calls") }
        settle()
    }

    private fun asked(op: String): Boolean = host.sent.any { it["op"]?.jsonPrimitive?.content == op }

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
        fontScale: Float = 1f,
        keyboard: Boolean = false,
        steps: ProjectsScreenshotTest.Screen.() -> Unit = {},
    ) {
        val activity = compose.activity
        // Animations off, as "Remove animations" sets it, so a capture does not catch a page mid-move.
        Settings.Global.putFloat(activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        if (fontScale != 1f) {
            val resources = activity.resources
            val config = Configuration(resources.configuration).apply { this.fontScale = fontScale }
            @Suppress("DEPRECATION")
            resources.updateConfiguration(config, resources.displayMetrics)
        }
        keyboardShown = keyboard
        host = AuditHost().connect()
        val model = MainViewModel(activity.application, host.client)
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

        /** A patch of 420 changed lines in one file. */
        val LONG_PATCH: String = buildString {
            append("diff --git a/shared/Backoff.kt b/shared/Backoff.kt\n")
            append("--- a/shared/Backoff.kt\n+++ b/shared/Backoff.kt\n")
            append("@@ -1,210 +1,210 @@\n")
            for (i in 1..210) {
                append("-val step$i = $i * 1_000L\n")
                append("+val step$i = minOf(2_000L shl $i, 120_000L)\n")
            }
        }
    }
}
