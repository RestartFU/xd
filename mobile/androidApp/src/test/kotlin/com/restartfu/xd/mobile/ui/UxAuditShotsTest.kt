package com.restartfu.xd.mobile.ui

import android.content.res.Configuration
import android.graphics.Bitmap
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.performClick
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.net.Link
import java.io.File
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Captures the surfaces the accessibility and conformance work changed, from
 * the whole app against [AuditHost]. With `-Pxd.shots=<dir>` it writes
 * `android-<surface>-<theme>.png` there.
 *
 * It asserts nothing about what the app says, and uses only what the app had
 * before that work, so the same file run on the commit before it gives the
 * matching "before" captures.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxAuditShotsTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** A session in each status, and a closed project gathering three. */
    @Test fun projectsDark() = shoot("android-projects-dark", MinimalThemePreset.DARK)

    @Test fun projectsLight() = shoot("android-projects-light", MinimalThemePreset.LIGHT)

    @Test fun projectsWhileReconnecting() =
        shoot("android-projects-reconnecting-light", MinimalThemePreset.LIGHT, link = DOWN)

    @Test fun projectsWithEnlargedText() =
        shoot("android-projects-fontscale2-dark", MinimalThemePreset.DARK, fontScale = 2f)

    @Test fun chatWhileReconnecting() = shoot("android-chat-reconnecting-dark", MinimalThemePreset.DARK, link = DOWN) {
        openChat()
    }

    @Test fun chatWhileFirstConnecting() =
        shoot("android-chat-connecting-dark", MinimalThemePreset.DARK, link = Link.Connecting) { openChat() }

    @Test fun chatWithToolRowsDark() = shoot("android-chat-tools-dark", MinimalThemePreset.DARK) {
        openChat()
        click("3 tool calls")
    }

    @Test fun chatWithToolRowsLight() = shoot("android-chat-tools-light", MinimalThemePreset.LIGHT) {
        openChat()
        click("3 tool calls")
    }

    @Test fun chatWithEnlargedText() =
        shoot("android-chat-fontscale2-dark", MinimalThemePreset.DARK, fontScale = 2f) { openChat() }

    /**
     * The title's menu of the session's chats. A menu's window is laid out
     * only once something draws it, so it is drawn once before the capture.
     */
    @Test fun chatMenu() = shoot("android-chat-menu-dark", MinimalThemePreset.DARK) {
        openChat()
        click("Fix reconnect backoff")
        repeat(3) {
            capture()
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    @Test @Config(qualifiers = "w840dp-h1200dp-320dpi")
    fun tabletChat() = shoot("android-tablet-chat-dark", MinimalThemePreset.DARK) { openChat() }

    @Test fun filesPane() = shoot("android-files-dark", MinimalThemePreset.DARK) {
        openChat()
        click("Workspace view: Chat")
        click("Files")
        await { shows("mobile") }
        click("mobile")
        await { shows("androidApp") }
        click("README.md")
        // The file's editor, with its Save, is up.
        await { shows("Save") }
        // Back to the tree, with the file's tab beside it: the tab named Files
        // is the lower of the two things that say so, under the header's view button.
        val named = compose.onAllNodes(hasText("Files"), useUnmergedTree = true)
        val tab = named.fetchSemanticsNodes().withIndex().maxBy { it.value.boundsInRoot.top }.index
        named[tab].performClick()
        settle()
    }

    @Test fun terminalLight() = shoot("android-terminal-light", MinimalThemePreset.LIGHT) { openTerminal() }

    @Test fun terminalDark() = shoot("android-terminal-dark", MinimalThemePreset.DARK) { openTerminal() }

    /** Settings › Devices, and what follows a tap on its one action, where a question does. */
    @Test fun settingsDevices() = shoot("android-settings-forget-dark", MinimalThemePreset.DARK) {
        click("Settings")
        await { shows("Devices") }
        click("Devices")
        settle()
        if (shows("Forget…")) click("Forget…")
        repeat(3) {
            ShadowLooper.idleMainLooper()
            settle()
        }
    }

    private lateinit var host: AuditHost

    private fun ProjectsScreenshotTest.Screen.openChat() {
        click("Reconnect backoff")
        await { shows("3 tool calls") }
        settle()
    }

    private fun ProjectsScreenshotTest.Screen.openTerminal() {
        click("Terminal")
        await { asked("terminal-resize") }
        repeat(3) { settle() }
    }

    /** Whether the app has sent [op], with [action] when one is given. */
    private fun asked(op: String, action: String? = null): Boolean = host.sent.any { request ->
        request["op"]?.jsonPrimitive?.content == op &&
            (action == null || request["action"]?.jsonPrimitive?.content == action)
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
        /** Shown in place of the real link, to capture a page while the host is out of reach. */
        link: Link? = null,
        fontScale: Float = 1f,
        steps: ProjectsScreenshotTest.Screen.() -> Unit = {},
    ) {
        val activity = compose.activity
        // Animations off, as "Remove animations" sets it, so a capture does not catch a pulse mid-beat.
        Settings.Global.putFloat(activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        // Through the configuration, as a phone with enlarged text has it.
        if (fontScale != 1f) {
            val resources = activity.resources
            val config = Configuration(resources.configuration).apply { this.fontScale = fontScale }
            @Suppress("DEPRECATION")
            resources.updateConfiguration(config, resources.displayMetrics)
        }
        host = AuditHost().connect()
        val model = MainViewModel(activity.application, host.client)
        val settings = MobileSettings(activity)
        settings.setTheme(theme)
        // Closed, so its row gathers the marks of the sessions inside.
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val live by model.client.link.collectAsStateWithLifecycle()
            XdRoot(theme) { MinimalMobileApp(model, link ?: live, settings) }
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
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { screen.capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }

    private companion object {
        val DOWN = Link.Down("Connection refused", 15_000)
    }
}
