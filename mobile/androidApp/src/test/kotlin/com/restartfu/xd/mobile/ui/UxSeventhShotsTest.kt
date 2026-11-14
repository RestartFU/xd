package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
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
 * Captures the surfaces the seventh audit's fixes changed, from the whole app
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
class UxSeventhShotsTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** The narrowest window that shows a tool beside the chat: the rail beside a session. */
    @Test @Config(qualifiers = SMALL_TABLET)
    fun smallTabletChat() = shoot("android-tablet-chat-840x600-dark", MinimalThemePreset.DARK) {
        openChat()
        click("Files")
        await { shows("mobile") }
    }

    @Test @Config(qualifiers = TABLET)
    fun tabletChatLight() = shoot("android-tablet-chat-wide-light", MinimalThemePreset.LIGHT) { openChat() }

    @Test @Config(qualifiers = LANDSCAPE_PHONE)
    fun chatOnALandscapePhone() = shoot("android-chat-landscape-dark", MinimalThemePreset.DARK) { openChat() }

    /** A tablet chat's tabs, the open one selected. */
    @Test @Config(qualifiers = SMALL_TABLET)
    fun tabletChatTabsDark() = shoot("android-tablet-chat-tabs-dark", MinimalThemePreset.DARK) { openChat() }

    @Test @Config(qualifiers = SMALL_TABLET)
    fun tabletChatTabsLight() = shoot("android-tablet-chat-tabs-light", MinimalThemePreset.LIGHT) { openChat() }

    /** Just after a queued message was dropped from its dialog. */
    @Test fun queuedMessageDropped() = shoot("android-queued-removed-dark", MinimalThemePreset.DARK) {
        click("Reconnect backoff")
        await { shows(QUEUED) }
        repeat(2) { settle() }
        click(QUEUED)
        repeat(3) {
            capture()
            ShadowLooper.idleMainLooper()
            settle()
        }
        click("Drop")
        await { asked("drop-queue") }
        repeat(6) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    /** The global terminal, which draws the sixteen ANSI colours. */
    @Test fun terminalDark() = shoot("android-terminal-dark", MinimalThemePreset.DARK) {
        click("Terminal")
        await { asked("terminal-resize") }
        repeat(3) { settle() }
    }

    @Test fun terminalLight() = shoot("android-terminal-light", MinimalThemePreset.LIGHT) {
        click("Terminal")
        await { asked("terminal-resize") }
        repeat(3) { settle() }
    }

    private lateinit var host: AuditHost

    private fun ProjectsScreenshotTest.Screen.openChat() {
        click("Reconnect backoff")
        await { shows("Backoff now starts at 2 s and grows to a 2 min cap. All 359 shared tests pass.") }
        repeat(2) { settle() }
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

    private fun shoot(name: String, theme: MinimalThemePreset, steps: ProjectsScreenshotTest.Screen.() -> Unit = {}) {
        val activity = compose.activity
        // Animations off, as "Remove animations" sets it, so a capture does not catch a page mid-move.
        Settings.Global.putFloat(activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        host = AuditHost(override = ::queued, afterReply = { op ->
            if (op == "drop-queue") """{"event":"queued","chat":"c1","queue":["$LATER"],"queue_revision":2}""" else null
        }).connect()
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

    private fun capture(screen: ProjectsScreenshotTest.Screen, name: String) {
        val dir = out ?: return
        dir.mkdirs()
        File(dir, "$name.png").outputStream().use { screen.capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
    }

    private companion object {
        const val SMALL_TABLET = "w840dp-h600dp-320dpi"
        const val TABLET = "w1280dp-h800dp-320dpi"
        const val LANDSCAPE_PHONE = "w780dp-h360dp-420dpi"
        const val QUEUED = "Also bump the version"
        const val LATER = "Then update the notes"

        /** The open chat, with two messages queued behind its turn. */
        fun queued(op: String?, request: JsonObject): String? {
            if (op != "chat" || (request["chat"] as? JsonPrimitive)?.content != "c1") return null
            return AuditHost.reply(op, request)
                .replace(""""queue":[]""", """"queue":["$QUEUED","$LATER"],"queue_revision":1""")
        }
    }
}
