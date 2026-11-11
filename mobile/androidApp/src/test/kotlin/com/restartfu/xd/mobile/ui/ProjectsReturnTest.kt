package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onLast
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipe
import androidx.compose.ui.geometry.Offset
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
 * Back from a chat returns to the same place in the projects list, not its top.
 *
 * With `-Pxd.shots=<dir>` it writes PNG captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class ProjectsReturnTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    @Test
    fun backFromAChatKeepsThePlaceInTheProjects() = withApp {
        waitFor { shows(title(1)) }
        // The project is one row holding every session, so scroll as a reader would.
        repeat(20) {
            if (!onScreen(title(SESSIONS))) {
                compose.onNode(hasScrollToIndexAction()).performTouchInput {
                    swipe(Offset(centerX, bottom - 200f), Offset(centerX, top + 200f), durationMillis = 400)
                }
                settle()
            }
        }
        waitFor { onScreen(title(SESSIONS)) }
        check(!onScreen(title(1))) { "Scrolling down left the first session on screen" }
        shoot("projects-scrolled")
        // The last session is under the floating navigation bar; open one above it.
        click(title(OPENED))
        waitFor { shows("Back") }
        click("Back")
        waitFor { !shows("Back") }
        shoot("projects-after-back")
        check(onScreen(title(OPENED))) { "Back from the chat did not show the session it was opened from" }
        check(!onScreen(title(1))) { "Back from the chat returned to the top of the projects" }
    }

    private lateinit var model: MainViewModel

    /** Starts the whole app against the scripted host, then runs [steps]. */
    private fun withApp(steps: () -> Unit) {
        val activity = compose.activity
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        model = MainViewModel(activity.application, XdClient({ ScriptedSocket().also { socket = it } }, NoCredentials, scope))
        val connected = runBlocking { model.client.connect(connection) }
        check(connected is ConnectResult.Success) { "Could not reach the scripted host: $connected" }
        val settings = MobileSettings(activity)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val link by model.client.link.collectAsStateWithLifecycle()
            CompositionLocalProvider(LocalXdSignals provides xdSignals(MinimalThemePreset.DARK)) {
                MaterialTheme(colorScheme = minimalColors(MinimalThemePreset.DARK), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) { MinimalMobileApp(model, link, settings) }
                }
            }
        }
        waitFor { model.client.tree.value.chats.isNotEmpty() }
        try {
            steps()
        } finally {
            scope.cancel()
        }
    }

    private fun settle() {
        repeat(10) {
            Thread.sleep(15)
            compose.mainClock.advanceTimeBy(50)
        }
    }

    private fun waitFor(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 60_000
        while (!condition()) {
            check(System.currentTimeMillis() < deadline) { "Timed out waiting" }
            settle()
        }
        settle()
    }

    private fun matching(label: String) = compose.onAllNodes(
        hasText(label, substring = true) or hasContentDescription(label),
        useUnmergedTree = true,
    )

    private fun shows(label: String) = matching(label).fetchSemanticsNodes().isNotEmpty()

    /**
     * Whether the text [label] is inside the list's viewport. Every session of
     * a project is in one row, so the text exists while it is scrolled away.
     */
    private fun onScreen(label: String): Boolean {
        if (!shows(label)) return false
        val list = compose.onNode(hasScrollToIndexAction()).fetchSemanticsNode().boundsInRoot
        return matching(label).fetchSemanticsNodes().any {
            val top = it.positionInRoot.y
            top + it.size.height > list.top && top < list.bottom
        }
    }

    private fun click(label: String) {
        matching(label).onLast().performClick()
        settle()
    }

    private fun shoot(name: String) {
        val dir = out ?: return
        dir.mkdirs()
        ShadowLooper.idleMainLooper()
        val screen = compose.activity.window.decorView
        val bitmap = Bitmap.createBitmap(screen.width, screen.height, Bitmap.Config.ARGB_8888)
        screen.draw(Canvas(bitmap))
        File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
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
                val op = request["op"]?.jsonPrimitive?.content
                val body = reply(op).removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"folders":[{"id":"xd","name":"xd"}],""" +
                """"sessions":[""" + (1..SESSIONS).joinToString(",") { n ->
                    """{"id":"s$n","folder":"xd","title":"${title(n)}","chats":["c$n"],"terminal_chat":"session:s$n"}"""
                } + """],"chats":[""" + (1..SESSIONS).joinToString(",") { n ->
                    """{"id":"c$n","folder":"xd","title":"${title(n)}","backend":"claude","working":false,"session":"s$n"}"""
                } + "]}"
            "chat" -> """{"ok":true,"title":"Chat","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":false,"label":"Claude","items":[],"new_worktree":false,""" +
                """"has_messages":true,"model":"claude-opus-5-5","effort":"high"}"""
            "messages" -> """{"ok":true,"total_messages":1,"last_message_id":1,"messages":[""" +
                """{"id":1,"role":"assistant","content":"Done.","at":1762500000}]}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        /** More sessions than fit on the screen. */
        const val SESSIONS = 30

        /** The session the test opens, near the end of the list. */
        const val OPENED = 27

        fun title(n: Int) = "Session number $n of the list"

        @Volatile var socket: ScriptedSocket? = null
    }
}
