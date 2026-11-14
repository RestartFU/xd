package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.mobile.XdMobileApp
import com.restartfu.xd.net.Link
import com.restartfu.xd.net.SocketFailureKind
import java.io.File
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Captures the surfaces the sixth audit's fixes changed, from the whole app
 * against [AuditHost]. With `-Pxd.shots=<dir>` it writes
 * `android-<surface>-<theme>.png` there.
 *
 * It asserts nothing about what the app says, and uses only what the app had
 * before those fixes (bar [XdMobileApp] being visible to tests), so the same
 * file run on the commit before them gives the matching "before" captures.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class UxSixthShotsTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** The floating tab bar on Projects, with Projects selected. */
    @Test fun tabBarDark() = shoot("android-tab-bar-dark", MinimalThemePreset.DARK)

    @Test fun tabBarLight() = shoot("android-tab-bar-light", MinimalThemePreset.LIGHT)

    /** New session on a tablet, opened from a project's row. */
    @Test @Config(qualifiers = TABLET)
    fun newSessionOnATablet() = shoot("android-new-session-tablet-dark", MinimalThemePreset.DARK) {
        click("New session in xd")
        repeat(4) { idle() }
    }

    @Test @Config(qualifiers = TABLET)
    fun newSessionOnATabletLight() = shoot("android-new-session-tablet-light", MinimalThemePreset.LIGHT) {
        click("New session in xd")
        repeat(4) { idle() }
    }

    /** The host refuses this device after an operation failed: the fatal screen. */
    @Test fun fatalAfterAnOperationError() = shoot("android-fatal-after-error-dark", MinimalThemePreset.DARK, whole = true) {
        compose.runOnUiThread { model.deleteChat("c9") }
        await { shows(REFUSAL) }
        repeat(3) { idle() }
        host.drop(SocketFailureKind.AUTHENTICATION, "This device was removed on the host. Pair it again.")
        await { model.client.link.value is Link.Fatal }
        await { shows("Host connection failed") }
        repeat(3) { idle() }
    }

    private lateinit var host: AuditHost
    private lateinit var model: MainViewModel

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
        /** The activity's whole app, fatal screen included, not only the workspace. */
        whole: Boolean = false,
        steps: ProjectsScreenshotTest.Screen.() -> Unit = {},
    ) {
        val activity = compose.activity
        // Animations off, as "Remove animations" sets it, so a capture does not catch a page mid-move.
        Settings.Global.putFloat(activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        host = AuditHost(override = { op, _ ->
            if (op == "delete-chat") """{"ok":false,"error":"$REFUSAL"}""" else null
        }).connect()
        model = MainViewModel(activity.application, host.client)
        val settings = MobileSettings(activity)
        settings.setTheme(theme)
        settings.setFolderOpen("web", false)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            if (whole) {
                XdRoot(theme) { XdMobileApp(model, settings) }
            } else {
                val live by model.client.link.collectAsStateWithLifecycle()
                XdRoot(theme) { MinimalMobileApp(model, live, settings) }
            }
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
        File(dir, "$name.png").outputStream().use { screen.capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
    }

    private companion object {
        const val TABLET = "w1280dp-h800dp-320dpi"
        const val REFUSAL = "The host could not delete the chat."
    }
}
