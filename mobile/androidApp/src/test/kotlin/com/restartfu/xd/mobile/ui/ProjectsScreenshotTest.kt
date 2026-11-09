package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.util.Base64
import android.view.View
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.SemanticsNode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isRoot
import androidx.compose.ui.test.junit4.AndroidComposeTestRule
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onLast
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
import java.io.ByteArrayOutputStream
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
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.Rule
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * Renders the Projects page against a scripted host that keeps folders.
 *
 * With `-Pxd.shots=<dir>` it writes PNG captures there for design review.
 * Nothing in the script is working: a running turn's pulse never lets the
 * paused Robolectric clock settle once the list recomposes.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class ProjectsScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @Test fun projectsDark() = shoot("projects-dark", MinimalThemePreset.DARK)

    @Test fun projectsLight() = shoot("projects-light", MinimalThemePreset.LIGHT)

    @Test fun projectsOpenOnTheirOwn() = shoot("projects-open-dark", MinimalThemePreset.DARK) {
        // Every project starts open, so sessions in two projects show at once.
        check(shows("Reconnect backoff") && shows("Landing copy") && shows("No sessions yet"))
        click("xd")
        check(!shows("Reconnect backoff") && !shows("Usage page") && shows("Landing copy"))
        click("xd")
        check(shows("Reconnect backoff") && shows("Landing copy"))
    }

    @Test fun newSessionAsksForAProject() = shoot("new-session-picker-dark", MinimalThemePreset.DARK) {
        click("New session")
        check(shows("Choose a project") && shows("2 sessions") && shows("acme · 1 session") &&
            shows("acme / Clients · No sessions") && !shows("Create session"))
    }

    @Test fun pickingAProjectStartsItsSession() = shoot("new-session-chosen-dark", MinimalThemePreset.DARK) {
        click("New session")
        val before = SENT.size
        // Nothing waits on the next dialog: its title field never lets Compose go idle.
        // The checkouts it reads say which project it is for.
        click("acme / Clients · No sessions")
        check(placesRead(before) == listOf("shop")) { placesRead(before).toString() }
    }

    @Test fun anOpenProjectStartsASessionInItself() = shoot("new-session-here-dark", MinimalThemePreset.DARK) {
        check(shows("New session in web") && shows("New session in xd"))
        val before = SENT.size
        click("New session in web")
        check(placesRead(before) == listOf("web")) { placesRead(before).toString() }
    }

    /** The projects whose checkouts the app asked for since request [from]. */
    private fun placesRead(from: Int): List<String> = synchronized(SENT) { SENT.drop(from) }
        .map { Json.parseToJsonElement(it).jsonObject }
        .filter { it["op"]?.jsonPrimitive?.content == "folder-settings" }
        .mapNotNull { it["folder"]?.jsonPrimitive?.content }

    @Test fun newMenu() = shoot("new-menu-dark", MinimalThemePreset.DARK) {
        click("New")
        check(shows("New project") && shows("New folder"))
    }

    @Test fun newFolderDialog() = shoot("new-folder-dark", MinimalThemePreset.DARK) {
        click("New")
        // Nothing waits on the dialog itself: one holding a text field never
        // lets Compose go idle under Robolectric. The organization's request
        // is checked in the shared OpsTest and ProjectTreeTest.
        click("New folder")
    }

    @Test fun folderMenu() = shoot("folder-menu-dark", MinimalThemePreset.DARK) {
        // A top-level folder with no other folder to go into cannot move.
        click("acme actions")
        check(shows("New project here") && shows("Prompt shortcuts") && shows("Folder context") &&
            shows("Agent secrets") && shows("Delete folder") && !shows("Move to…"))
        pressBack()
        click("Clients actions")
        check(shows("New project here") && shows("Move to…") && shows("Delete folder"))
    }

    @Test fun moveDialog() = shoot("move-dark", MinimalThemePreset.DARK) {
        click("notes")
        click("notes actions")
        click("Move to…")
        click("Clients")
        check(shows("Here now"))
    }

    @Test fun closingAFolderHidesWhatItHolds() = shoot("folder-closed-dark", MinimalThemePreset.DARK) {
        check(shows("web") && shows("shop"))
        click("acme")
        check(!shows("web") && !shows("shop") && !shows("Clients"))
        click("acme")
        check(shows("web") && shows("shop"))
    }

    @Test fun movingSendsTheChosenFolder() = shoot("moved-dark", MinimalThemePreset.DARK) {
        click("notes")
        click("notes actions")
        click("Move to…")
        click("Clients")
        click("Move")
        val move = SENT.last { it.contains("\"move-folder\"") }
        check(move.contains("\"folder\":\"notes\"") && move.contains("\"parent\":\"clients\"")) { move }
    }

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** Opens the page, lets [steps] act on it, and captures the result. */
    private fun shoot(name: String, theme: MinimalThemePreset, steps: Screen.() -> Unit = {}) {
        val activity = compose.activity
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val model = MainViewModel(activity.application, XdClient({ ScriptedSocket() }, NoCredentials, scope))
        val connected = runBlocking { model.client.connect(connection) }
        check(connected is ConnectResult.Success) { "Could not reach the scripted host: $connected" }
        val settings = MobileSettings(activity)
        // Time moves only when the test says, so running animations cannot keep Compose busy.
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val link by model.client.link.collectAsStateWithLifecycle()
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) { MinimalMobileApp(model, link, settings) }
                }
            }
        }
        val screen = Screen(compose, activity)
        // The scripted host answers on its own threads, so wait for the tree and avatars.
        val deadline = System.currentTimeMillis() + 60_000
        while (
            (model.client.tree.value.folders.isEmpty() || model.client.ownerIcons.value.size < 2) &&
            System.currentTimeMillis() < deadline
        ) screen.settle()
        screen.settle()
        try {
            screen.steps()
        } catch (failure: Throwable) {
            // What was on screen when it went wrong, to see why.
            out?.let { dir ->
                dir.mkdirs()
                File(dir, "$name-failed.png").outputStream().use { screen.capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
            }
            // A dialog holding a text field never lets Compose go idle, so its labels may not be readable.
            val shown = runCatching { screen.labels().toString() }.getOrDefault("(screen not idle)")
            throw AssertionError("${failure.message}; showing $shown", failure)
        }
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { screen.capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
        scope.cancel()
    }

    /** Drives the page as a person would by touch. */
    class Screen(
        private val compose: AndroidComposeTestRule<*, *>,
        private val activity: ComponentActivity,
    ) {
        fun settle() {
            repeat(10) {
                Thread.sleep(15)
                compose.mainClock.advanceTimeBy(50)
            }
        }

        private fun matching(label: String) = compose.onAllNodes(
            hasText(label) or hasContentDescription(label),
            useUnmergedTree = true,
        )

        fun labels(): List<String> = compose.onAllNodes(isRoot()).fetchSemanticsNodes().flatMap { root ->
            fun walk(node: SemanticsNode): List<SemanticsNode> = listOf(node) + node.children.flatMap(::walk)
            walk(root)
        }.mapNotNull { node ->
            node.config.getOrNull(SemanticsProperties.Text)?.joinToString { it.text }
                ?: node.config.getOrNull(SemanticsProperties.ContentDescription)?.joinToString()
        }

        fun shows(label: String): Boolean = matching(label).fetchSemanticsNodes().isNotEmpty()

        fun click(label: String) {
            matching(label).onLast().performClick()
            settle()
        }

        /**
         * Sets a field's text without focusing it: a focused field's blinking
         * cursor never lets Compose go idle under the test clock.
         */


        fun pressBack() {
            compose.runOnUiThread { activity.onBackPressedDispatcher.onBackPressed() }
            ShadowLooper.idleMainLooper()
            settle()
        }

        /** This activity's window and the dialogs and menus over it. */
        private fun windows(): List<View> {
            val global = Class.forName("android.view.WindowManagerGlobal")
            val instance = global.getMethod("getInstance").invoke(null)
            val field = global.getDeclaredField("mViews").apply { isAccessible = true }
            @Suppress("UNCHECKED_CAST")
            val views = (field.get(instance) as List<View>).toList()
            val screen = activity.window.decorView
            // Earlier tests' windows can linger; only those after this activity's own are its.
            return views.drop(views.indexOf(screen).coerceAtLeast(0))
        }

        /** The activity with any dialogs and menus drawn over it where they sit. */
        fun capture(): Bitmap {
            ShadowLooper.idleMainLooper()
            val views = windows()
            val screen = activity.window.decorView
            val bitmap = Bitmap.createBitmap(screen.width, screen.height, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap)
            screen.draw(canvas)
            for (view in views) {
                if (view === screen || view.width == 0) continue
                val params = view.layoutParams as? android.view.WindowManager.LayoutParams
                val dialog = params?.type == android.view.WindowManager.LayoutParams.TYPE_APPLICATION
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
                SENT += line
                val request = Json.parseToJsonElement(line).jsonObject
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val op = request["op"]?.jsonPrimitive?.content
                val body = reply(op, request["owner"]?.jsonPrimitive?.content)
                    .removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?, owner: String?): String = when (op) {
            "tree" -> TREE
            "owner-icon" -> """{"ok":true,"owner":"$owner","mime":"image/png","data":"${avatar(owner.orEmpty())}"}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        /** Every request the app sent, in order. */
        val SENT: MutableList<String> = java.util.Collections.synchronizedList(mutableListOf())

        /** An organization holding a project and a folder of client sites, beside two plain projects. */
        val TREE = """{"ok":true,"settlement":true,"folder_reorder":true,"folder_groups":true,"folders":[""" +
            """{"id":"xd","name":"xd","owner":"RestartFU"},""" +
            """{"id":"acme","name":"acme","group":true,"owner":"acme"},""" +
            """{"id":"web","name":"web","parent":"acme","owner":"acme"},""" +
            """{"id":"clients","name":"Clients","parent":"acme","group":true},""" +
            """{"id":"shop","name":"shop","parent":"clients"},""" +
            """{"id":"notes","name":"notes"}],""" +
            """"sessions":[{"id":"s1","folder":"xd","title":"Reconnect backoff","chats":["c1"],"terminal_chat":"session:s1"},""" +
            """{"id":"s2","folder":"xd","title":"Usage page","chats":["c2"],"terminal_chat":"session:s2"},""" +
            """{"id":"s3","folder":"web","title":"Landing copy","chats":["c3"],"terminal_chat":"session:s3"}],""" +
            """"chats":[{"id":"c1","folder":"xd","title":"Reconnect backoff","backend":"claude","working":false,"session":"s1"},""" +
            """{"id":"c2","folder":"xd","title":"Usage page","backend":"codex","working":false,"session":"s2"},""" +
            """{"id":"c3","folder":"web","title":"Landing copy","backend":"codex","working":false,"session":"s3"}]}"""

        /** A plain stand-in avatar: the account's initial on a colour of its own. */
        fun avatar(owner: String): String {
            val bitmap = Bitmap.createBitmap(64, 64, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap)
            canvas.drawColor(if (owner == "acme") Color.rgb(0x3b, 0x82, 0xf6) else Color.rgb(0xe1, 0x6b, 0x3a))
            val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                color = Color.WHITE
                textSize = 40f
                textAlign = Paint.Align.CENTER
                isFakeBoldText = true
            }
            canvas.drawText(owner.take(1).uppercase(), 32f, 46f, paint)
            val bytes = ByteArrayOutputStream().also { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
            return Base64.encodeToString(bytes, Base64.NO_WRAP)
        }
    }
}
