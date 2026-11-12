package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.view.View
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.performTextReplacement
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.isNotSelected
import androidx.compose.ui.test.isSelectable
import androidx.compose.ui.test.isSelected
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.text.TextLayoutResult
import android.content.res.Configuration
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.performClick
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Opens the app's Settings against a scripted host and checks what it offers.
 * With `-Pxd.shots=<dir>` it writes a PNG capture there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class SettingsScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** Settings opens as its own page, listing its sections. */
    @Test fun homeDark() = shoot("settings-home-dark", MinimalThemePreset.DARK) {
        checkHome()
        // The page takes the focus, so keys never reach the workspace beneath it.
        compose.onNode(hasTestTag(SETTINGS_PAGE_TAG)).assertIsFocused()
    }

    @Test fun homeLight() = shoot("settings-home-light", MinimalThemePreset.LIGHT) { checkHome() }

    /** Both themes are offered, and the chosen one is the one marked. */
    @Test fun appearanceDark() = shoot("settings-appearance-dark", MinimalThemePreset.DARK) {
        open("Appearance")
        checkThemes(MinimalThemePreset.DARK)
    }

    @Test fun appearanceLight() = shoot("settings-appearance-light", MinimalThemePreset.LIGHT) {
        open("Appearance")
        checkThemes(MinimalThemePreset.LIGHT)
    }

    /** The themes still fit side by side on a narrow phone. */
    @Config(qualifiers = "w360dp-h780dp-420dpi")
    @Test fun appearanceNarrow() = shoot("settings-narrow", MinimalThemePreset.DARK) {
        open("Appearance")
        checkThemes(MinimalThemePreset.DARK)
    }

    /** With enlarged text the themes stack rather than clip their names. */
    @Config(qualifiers = "w360dp-h780dp-420dpi")
    @Test fun appearanceLargeText() = shoot("settings-large-text", MinimalThemePreset.DARK, fontScale = 2f) {
        open("Appearance")
        checkThemes(MinimalThemePreset.DARK)
    }

    /** A section opens as a page of its own, and Back returns to the list. */
    @Test fun aSectionOpensAndBackReturns() = shoot("settings-chats-dark", MinimalThemePreset.DARK) {
        open("Chats")
        check(shows("Resume chats after a restart") && shows("Prompt shortcuts"))
        check(!shows("Search settings") && !shows("Skip permission prompts"))
        capture("settings-chats-dark")
        compose.onNode(hasContentDescription("Back")).performClick()
        idle()
        checkHome()
    }

    /** Search finds settings in any section by their words, grouped under the section. */
    @Test fun searchFiltersEverySection() = shoot("settings-search-dark", MinimalThemePreset.DARK) {
        compose.onNode(hasSetTextAction()).performTextInput("settle")
        idle()
        check(shows("Settle merged sessions") && shows("Settle inactive sessions")) { "No settling rows" }
        check(shows("Cleanup"))
        check(!shows("Theme") && !shows("Resume chats after a restart") && !shows("Agents"))
        capture("settings-search-dark")
        compose.onNode(hasSetTextAction()).performTextReplacement("zzz")
        idle()
        check(shows("No settings match"))
        // A word that only a description shows still finds its setting.
        compose.onNode(hasSetTextAction()).performTextReplacement("inactivity")
        idle()
        check(shows("Keep sessions with a pull request")) { "Descriptions are not searched" }
        // The page's Back clears a search first, as system Back does, then leaves.
        compose.onNode(hasContentDescription("Back")).performClick()
        idle()
        checkHome()
        compose.onNode(hasContentDescription("Back")).performClick()
        idle()
        check(!shows("Search settings")) { "Back did not leave Settings" }
    }

    private fun checkHome() {
        for (section in listOf("Appearance", "Chats", "Agents", "Cleanup", "Devices", "Updates")) {
            check(shows(section)) { "No $section section" }
        }
        check(shows("Search settings"))
        // The rows themselves wait inside their sections.
        check(!shows("Skip permission prompts") && !shows("Read replies aloud"))
    }

    private fun open(section: String) {
        compose.onAllNodes(hasText(section)).onFirst().performClick()
        idle()
        check(compose.onAllNodes(hasText(section)).fetchSemanticsNodes().isNotEmpty())
    }

    private fun checkThemes(theme: MinimalThemePreset) {
        for (preset in MinimalThemePreset.entries) {
            // The card is one radio choice, marked only when it is the theme in use.
            compose.onNode(hasText(preset.label) and isSelectable())
                .assert(if (preset == theme) isSelected() else isNotSelected())
            val layout = mutableListOf<TextLayoutResult>()
            compose.onNode(hasText(preset.label) and hasClickAction().not(), useUnmergedTree = true)
                .fetchSemanticsNode().config[SemanticsActions.GetTextLayoutResult].action!!(layout)
            // The name's natural one-line width fits the width its card gives it.
            val text = layout.single()
            val natural = text.multiParagraph.intrinsics.maxIntrinsicWidth
            check(text.lineCount == 1 && natural <= text.layoutInput.constraints.maxWidth) {
                "${preset.label} does not fit its card: ${text.lineCount} lines, " +
                    "$natural px wide in ${text.layoutInput.constraints.maxWidth} px"
            }
        }
    }

    private fun idle() = repeat(5) {
        ShadowLooper.idleMainLooper()
        settle()
    }

    private fun capture(name: String) {
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }

    private fun shoot(
        name: String,
        theme: MinimalThemePreset,
        fontScale: Float = 1f,
        steps: () -> Unit,
    ) {
        val activity = compose.activity
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val model = MainViewModel(activity.application, XdClient({ ScriptedSocket() }, NoCredentials, scope))
        val connected = runBlocking { model.client.connect(connection) }
        check(connected is ConnectResult.Success) { "Could not reach the scripted host: $connected" }
        val settings = MobileSettings(activity)
        settings.setTheme(theme)
        // Through the configuration, as a phone with enlarged text has it.
        if (fontScale != 1f) {
            val resources = activity.resources
            val config = Configuration(resources.configuration).apply { this.fontScale = fontScale }
            @Suppress("DEPRECATION")
            resources.updateConfiguration(config, resources.displayMetrics)
        }
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val link by model.client.link.collectAsStateWithLifecycle()
            XdRoot(theme) { MinimalMobileApp(model, link, settings) }
        }
        val deadline = System.currentTimeMillis() + 60_000
        while (model.client.tree.value.folders.isEmpty() && System.currentTimeMillis() < deadline) settle()
        settle()
        var done = false
        try {
            compose.onAllNodes(hasContentDescription("Settings"), useUnmergedTree = true).onFirst().performClick()
            idle()
            steps()
            done = true
        } finally {
            // A test that captured its own moment keeps it; otherwise the last screen.
            if (!done || out?.let { File(it, "$name.png").exists() } != true) capture(name)
            scope.cancel()
        }
    }

    private fun settle() {
        repeat(10) {
            Thread.sleep(15)
            compose.mainClock.advanceTimeBy(50)
        }
    }

    private fun shows(text: String) =
        compose.onAllNodes(hasText(text, substring = true), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    /** The activity with the dialogs over it drawn where they sit. */
    private fun capture(): Bitmap {
        ShadowLooper.idleMainLooper()
        val activity = compose.activity
        val global = Class.forName("android.view.WindowManagerGlobal")
        val instance = global.getMethod("getInstance").invoke(null)
        val field = global.getDeclaredField("mViews").apply { isAccessible = true }
        @Suppress("UNCHECKED_CAST")
        val all = (field.get(instance) as List<View>).toList()
        val screen = activity.window.decorView
        val views = all.drop(all.indexOf(screen).coerceAtLeast(0))
        val bitmap = Bitmap.createBitmap(screen.width, screen.height, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        screen.draw(canvas)
        for (view in views) {
            if (view === screen || view.width == 0) continue
            val dialog = (view.layoutParams as? WindowManager.LayoutParams)?.type ==
                WindowManager.LayoutParams.TYPE_APPLICATION
            if (dialog) canvas.drawColor(Color.argb(82, 0, 0, 0))
            val location = IntArray(2).also(view::getLocationOnScreen)
            val (x, y) = if (dialog) {
                (screen.width - view.width) / 2f to (screen.height - view.height) / 2f
            } else {
                location[0].toFloat() to location[1].toFloat()
            }
            canvas.save()
            canvas.translate(x, y)
            view.draw(canvas)
            canvas.restore()
        }
        return bitmap
    }

    private object NoCredentials : CredentialStore {
        override suspend fun load(): StoredCredentials? = null
        override suspend fun save(credentials: StoredCredentials) = Unit
        override suspend fun clear() = Unit
    }

    private val connection = SshConnection(
        "host", 22, "dev", SshAuthentication.Password("x"),
        SshHostKey("ssh-ed25519", byteArrayOf(1), "SHA256:test"),
    )

    /** Answers each request from a fixed script, echoing its request id. */
    private class ScriptedSocket : PlatformSocket {
        private var listener: PlatformSocketListener? = null

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val body = reply(request["op"]?.jsonPrimitive?.content)
                    .removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> TREE
            "auto-settle-settings" -> """{"ok":true,"after_days":3,"on_merge":true,"keep_pull_requests":false}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        val TREE = """{"ok":true,"folders":[{"id":"xd","name":"xd"}],"sessions":[],"chats":[],"settlement":true}"""
    }
}
