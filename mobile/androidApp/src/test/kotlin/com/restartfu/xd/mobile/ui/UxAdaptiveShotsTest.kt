package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.provider.Settings
import android.util.Base64
import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.performClick
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import java.io.ByteArrayOutputStream
import java.io.File
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Captures the surfaces the adaptive-layout, theming and conformance work
 * changed, from the whole app against [AuditHost], at the window sizes that
 * work was done for. With `-Pxd.shots=<dir>` it writes
 * `android-<surface>-<theme>.png` there.
 *
 * It asserts nothing about what the app says, and uses only what the app had
 * before that work, so the same file run on the commit before it gives the
 * matching "before" captures.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxAdaptiveShotsTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** A phone on its side: wide enough for a tablet's width, far too short for its header. */
    @Test @Config(qualifiers = LANDSCAPE_PHONE)
    fun chatOnALandscapePhone() = shoot("android-chat-landscape-dark", MinimalThemePreset.DARK) { openChat() }

    @Test @Config(qualifiers = LANDSCAPE_PHONE)
    fun projectsOnALandscapePhone() = shoot("android-projects-landscape-dark", MinimalThemePreset.DARK)

    @Test @Config(qualifiers = TABLET)
    fun tabletChat() = shoot("android-tablet-chat-wide-dark", MinimalThemePreset.DARK) { openChat() }

    @Test @Config(qualifiers = TABLET)
    fun tabletChatLight() = shoot("android-tablet-chat-wide-light", MinimalThemePreset.LIGHT) { openChat() }

    @Test @Config(qualifiers = TABLET)
    fun tabletChatWithFiles() = shoot("android-tablet-chat-files-dark", MinimalThemePreset.DARK) {
        openChat()
        click("Files")
        await { shows("mobile") }
    }

    @Test @Config(qualifiers = TABLET)
    fun tabletChatWithChanges() = shoot("android-tablet-chat-changes-light", MinimalThemePreset.LIGHT) {
        openChat()
        click("Changes")
        await { asked("diff-read") }
        // The pane reads the patch, then colours it away from the main thread.
        repeat(8) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    @Test @Config(qualifiers = TABLET)
    fun tabletChatWithTerminal() = shoot("android-tablet-chat-terminal-dark", MinimalThemePreset.DARK) {
        openChat()
        click("Terminal")
        await { asked("terminal-resize") }
        repeat(3) { settle() }
    }

    @Test @Config(qualifiers = TABLET)
    fun tabletProjects() = shoot("android-tablet-projects-dark", MinimalThemePreset.DARK)

    @Test @Config(qualifiers = TABLET)
    fun tabletSettings() = shoot("android-tablet-settings-dark", MinimalThemePreset.DARK) {
        click("Settings")
        await { shows("Appearance") }
    }

    @Test @Config(qualifiers = TABLET)
    fun tabletSettingsSection() = shoot("android-tablet-settings-appearance-light", MinimalThemePreset.LIGHT) {
        click("Settings")
        await { shows("Appearance") }
        click("Appearance")
        repeat(2) { settle() }
    }

    /** The narrowest window that shows a tool beside the chat. */
    @Test @Config(qualifiers = "w840dp-h600dp-320dpi")
    fun smallTabletChat() = shoot("android-tablet-chat-840x600-dark", MinimalThemePreset.DARK) {
        openChat()
        click("Files")
        await { shows("mobile") }
    }

    /** A tablet upright: too narrow for two panes, tall enough for the tablet header. */
    @Test @Config(qualifiers = "w600dp-h960dp-320dpi")
    fun uprightTabletChat() = shoot("android-tablet-chat-600x960-dark", MinimalThemePreset.DARK) { openChat() }

    @Test fun pairingDark() = shootPairing("android-pairing-dark", MinimalThemePreset.DARK)

    @Test fun pairingLight() = shootPairing("android-pairing-light", MinimalThemePreset.LIGHT)

    @Test @Config(qualifiers = TABLET)
    fun pairingOnATablet() = shootPairing("android-tablet-pairing-dark", MinimalThemePreset.DARK)

    /** A reply's picture, opened by tapping it. */
    @Test fun imageViewer() = shoot("android-image-viewer-dark", MinimalThemePreset.DARK, pictures = true) {
        openPicture()
    }

    @Test fun imageViewerLight() = shoot("android-image-viewer-light", MinimalThemePreset.LIGHT, pictures = true) {
        openPicture()
    }

    private lateinit var host: AuditHost

    private fun ProjectsScreenshotTest.Screen.openChat() {
        click("Reconnect backoff")
        // Its last line, which a window too short for the whole reply still shows.
        await { shows("Backoff now starts at 2 s and grows to a 2 min cap. All 359 shared tests pass.") }
        repeat(2) { settle() }
    }

    private fun ProjectsScreenshotTest.Screen.openPicture() {
        click("Reconnect backoff")
        val picture = { compose.onAllNodes(hasContentDescription("Image", substring = true), useUnmergedTree = true) }
        await { picture().fetchSemanticsNodes().isNotEmpty() }
        picture().onFirst().performClick()
        // A dialog's window is laid out only once something draws it.
        repeat(3) {
            capture()
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    private fun asked(op: String): Boolean = host.sent.any { (it["op"] as? JsonPrimitive)?.content == op }

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
        pictures: Boolean = false,
        steps: ProjectsScreenshotTest.Screen.() -> Unit = {},
    ) {
        val activity = compose.activity
        // Animations off, as "Remove animations" sets it, so a capture does not catch one mid-way.
        Settings.Global.putFloat(activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        host = AuditHost(override = { op, request -> scripted(op, request, pictures) }).connect()
        val model = MainViewModel(activity.application, host.client)
        val settings = MobileSettings(activity)
        settings.setTheme(theme)
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val link by model.client.link.collectAsStateWithLifecycle()
            XdRoot(theme) { MinimalMobileApp(model, link, settings) }
        }
        val screen = ProjectsScreenshotTest.Screen(compose, activity)
        try {
            screen.await { model.client.tree.value.folders.isNotEmpty() }
            host.pushSignals()
            screen.await {
                "c8" in model.client.waitingChats.value && "c9" in model.client.unreadChats.value
            }
            screen.settle()
            screen.steps()
        } catch (failure: Throwable) {
            capture(screen, "$name-failed")
            throw failure
        }
        capture(screen, name)
        host.close()
    }

    /** The pairing form, as a device with no pairing shows it. */
    private fun shootPairing(name: String, theme: MinimalThemePreset) {
        val activity = compose.activity
        host = AuditHost()
        val model = MainViewModel(activity.application, host.client)
        compose.mainClock.autoAdvance = false
        compose.setContent { XdRoot(theme) { ConnectScreen(model) } }
        val screen = ProjectsScreenshotTest.Screen(compose, activity)
        repeat(2) { screen.settle() }
        capture(screen, name)
        host.close()
    }

    private fun capture(screen: ProjectsScreenshotTest.Screen, name: String) {
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { screen.capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }

    private companion object {
        const val LANDSCAPE_PHONE = "w780dp-h360dp-420dpi"
        const val TABLET = "w1280dp-h800dp-320dpi"

        private const val PATCH = "diff --git a/mobile/shared/Backoff.kt b/mobile/shared/Backoff.kt\n" +
            "--- a/mobile/shared/Backoff.kt\n+++ b/mobile/shared/Backoff.kt\n" +
            "@@ -3,5 +3,7 @@ class Backoff {\n" +
            "     private var attempt = 0\n" +
            "-    fun next(): Long = 2_000\n" +
            "+    fun next(): Long = (2_000L shl attempt++).coerceAtMost(CAP)\n" +
            "+\n" +
            "+    fun reset() { attempt = 0 }\n" +
            " }\n"

        /** What this test's host answers differently from [AuditHost]'s script. */
        fun scripted(op: String?, request: JsonObject, pictures: Boolean): String? = when {
            op == "diff-read" -> """{"ok":true,"output":${JsonPrimitive(PATCH)}}"""
            pictures && op == "image-read" -> """{"ok":true,"mime":"image/png","data":"${picture()}"}"""
            pictures && op == "messages" && (request["chat"] as? JsonPrimitive)?.content == "c1" -> messages(
                "user" to "What does the retry curve look like now?",
                "assistant" to "Here is the delay before each attempt, capped at two minutes.\n\n" +
                    "[image: /tmp/xd/backoff-curve.png]\n\nIt doubles from 2 s and flattens at the cap.",
            )
            else -> null
        }

        private fun messages(vararg rows: Pair<String, String>): String =
            """{"ok":true,"total_messages":${rows.size},"last_message_id":${rows.size},"messages":[""" +
                rows.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"

        /** A drawn chart, so the capture shows a picture rather than a flat block. */
        private fun picture(): String {
            val bitmap = Bitmap.createBitmap(960, 540, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap)
            canvas.drawColor(0xFF1B1E2C.toInt())
            val grid = Paint().apply { color = 0xFF3A3F55.toInt(); strokeWidth = 2f }
            for (step in 1..5) canvas.drawLine(60f, 480f - step * 80f, 900f, 480f - step * 80f, grid)
            val bar = Paint().apply { color = 0xFF8B8FF8.toInt() }
            listOf(20f, 40f, 80f, 160f, 320f, 400f, 400f, 400f).forEachIndexed { index, height ->
                val left = 90f + index * 100f
                canvas.drawRect(left, 480f - height, left + 64f, 480f, bar)
            }
            val bytes = ByteArrayOutputStream().also { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
            return Base64.encodeToString(bytes, Base64.NO_WRAP)
        }
    }
}
