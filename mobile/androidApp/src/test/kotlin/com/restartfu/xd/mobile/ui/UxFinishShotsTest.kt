package com.restartfu.xd.mobile.ui

import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.hasScrollAction
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.performSemanticsAction
import androidx.compose.ui.text.AnnotatedString
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.net.Link
import java.io.File
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Captures the surfaces the fourth audit's fixes changed, from the whole app
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
class UxFinishShotsTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** A file with edits in it, and what a tap on its tab's close does. */
    @Test fun closingAnEditedFile() = shoot("android-file-close-edited-dark", MinimalThemePreset.DARK) {
        openChat()
        click("Workspace view: Chat")
        click("Files")
        await { shows("README.md") }
        click("README.md")
        await { shows("Save") }
        compose.onAllNodes(hasSetTextAction() and hasText("# xd", substring = true), useUnmergedTree = true).onFirst()
            .performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString("# xd\n\nA workspace for agents.\n")) }
        idle()
        click("Close README.md")
        idle()
    }

    /** Settings with its search field, whose edge a control needs. */
    @Test fun settingsSearchDark() = shoot("android-settings-search-field-dark", MinimalThemePreset.DARK) {
        click("Settings")
        await { shows("Search settings") }
        idle()
    }

    @Test fun settingsSearchLight() = shoot("android-settings-search-field-light", MinimalThemePreset.LIGHT) {
        click("Settings")
        await { shows("Search settings") }
        idle()
    }

    /** A search with many results, scrolled to its end, with the keyboard up. */
    @Test fun settingsSearchWithTheKeyboard() =
        shoot("android-settings-search-ime-dark", MinimalThemePreset.DARK, keyboard = true) {
            click("Settings")
            await { shows("Search settings") }
            compose.onAllNodes(hasSetTextAction(), useUnmergedTree = true).onFirst()
                .performSemanticsAction(SemanticsActions.SetText) { it(AnnotatedString("s")) }
            idle()
            showKeyboard(compose.activity, keyboardPx(), navigationPx())
            idle()
            compose.mainClock.autoAdvance = true
            compose.onAllNodes(hasScrollAction()).fetchSemanticsNodes().forEach { node ->
                compose.onNode(androidx.compose.ui.test.SemanticsMatcher("scroll ${node.id}") { it.id == node.id })
                    .performSemanticsAction(SemanticsActions.ScrollBy) { it(0f, 100_000f) }
            }
            compose.waitForIdle()
            compose.mainClock.autoAdvance = false
            idle()
        }

    /** The chat's composer, and the view button beside the title. */
    @Test fun composerDark() = shoot("android-chat-composer-dark", MinimalThemePreset.DARK) { openChat() }

    @Test fun composerLight() = shoot("android-chat-composer-light", MinimalThemePreset.LIGHT) { openChat() }

    /** The connection line while the host is out of reach. */
    @Test fun chatOffline() = shoot("android-chat-offline-dark", MinimalThemePreset.DARK, link = DOWN) { openChat() }

    /** New session on a wide tablet. */
    @Test @Config(qualifiers = "w1280dp-h800dp-160dpi")
    fun newSessionOnATablet() = shoot("android-new-session-tablet-dark", MinimalThemePreset.DARK) {
        click("New session in xd")
        repeat(6) { idle() }
    }

    /** New session for a project the host lists no checkouts or branches for. */
    @Test fun newSessionWithNoCheckouts() = shoot("android-new-session-no-checkouts-dark", MinimalThemePreset.DARK) {
        click("New session in xd")
        repeat(6) { idle() }
    }

    /** The tablet chat's header with the largest text Android offers. */
    @Test @Config(qualifiers = "w1280dp-h800dp-160dpi")
    fun tabletHeaderWithEnlargedText() =
        shoot("android-chat-tablet-fontscale2-dark", MinimalThemePreset.DARK, fontScale = 2f) { openChat() }

    /** The phone chat's header with the largest text Android offers. */
    @Test fun phoneHeaderWithEnlargedText() =
        shoot("android-chat-header-fontscale2-dark", MinimalThemePreset.DARK, fontScale = 2f) { openChat() }

    private lateinit var host: AuditHost
    private var keyboardShown = false

    private fun keyboardPx() = (KEYBOARD_DP * compose.activity.resources.displayMetrics.density).toInt()

    private fun navigationPx() = (NAVIGATION_DP * compose.activity.resources.displayMetrics.density).toInt()

    private fun ProjectsScreenshotTest.Screen.openChat() {
        click("Reconnect backoff")
        await { shows("3 tool calls") }
        settle()
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
        link: Link? = null,
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
            XdRoot(theme) { MinimalMobileApp(model, link ?: live, settings) }
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
        val DOWN = Link.Down("Connection refused", 15_000)
    }
}
