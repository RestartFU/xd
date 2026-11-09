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
import androidx.compose.ui.test.performScrollToIndex
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
import kotlinx.serialization.json.JsonPrimitive
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
 * Going into a chat shows its newest message: the end of a reply taller than
 * the screen, and again after the reader scrolled back, left, and returned.
 *
 * With `-Pxd.shots=<dir>` it writes PNG captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class ChatLandingTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    @Test
    fun goingIntoAChatLandsOnItsNewestMessage() {
        val activity = compose.activity
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val model = MainViewModel(activity.application, XdClient({ ScriptedSocket() }, NoCredentials, scope))
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

        click(TITLE)
        waitFor { shows(FIRST_LINE_OF_LATEST) || shows(LAST_LINE_OF_LATEST) }
        settle()
        shoot("chat-opened")
        check(endOfLatestOnScreen()) { "Opening the chat did not show the end of its newest reply" }

        // Back through the history, out to the projects, and in again.
        compose.onNode(hasScrollToIndexAction()).performScrollToIndex(0)
        settle()
        check(!shows(FIRST_LINE_OF_LATEST)) { "Scrolling back left the newest reply on screen" }
        click("Back")
        waitFor { !shows(FIRST_LINE_OF_LATEST) && !shows("Back") }
        click(TITLE)
        waitFor { shows("Back") }
        settle()
        shoot("chat-reopened")
        check(endOfLatestOnScreen()) { "Going back into the chat kept the old scroll position" }

        scope.cancel()
    }

    /**
     * Whether the newest reply's last line is inside the list's viewport. Its
     * text exists as soon as any of the reply is on screen, so its bounds are
     * what tell.
     */
    private fun endOfLatestOnScreen(): Boolean {
        val list = compose.onNode(hasScrollToIndexAction()).fetchSemanticsNode().boundsInRoot
        val ends = matching(LAST_LINE_OF_LATEST).fetchSemanticsNodes().map { it.boundsInRoot.bottom }
        return ends.any { it > list.top && it <= list.bottom + 1f }
    }

    private fun settle() {
        repeat(10) {
            Thread.sleep(15)
            compose.mainClock.advanceTimeBy(50)
        }
    }

    private fun waitFor(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 60_000
        while (!condition() && System.currentTimeMillis() < deadline) settle()
        settle()
    }

    private fun matching(label: String) = compose.onAllNodes(
        hasText(label, substring = true) or hasContentDescription(label),
        useUnmergedTree = true,
    )

    private fun shows(label: String) = matching(label).fetchSemanticsNodes().isNotEmpty()

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
                """"sessions":[{"id":"s1","folder":"xd","title":"$TITLE","chats":["c1"],"terminal_chat":"session:s1"}],""" +
                """"chats":[{"id":"c1","folder":"xd","title":"$TITLE","backend":"claude","working":false,"session":"s1"}]}"""
            "chat" -> """{"ok":true,"title":"$TITLE","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":false,"label":"Claude","items":[],"new_worktree":false,""" +
                """"has_messages":true,"model":"claude-opus-5-5","effort":"high"}"""
            "messages" -> """{"ok":true,"total_messages":${MESSAGES.size},"last_message_id":${MESSAGES.size},"messages":[""" +
                MESSAGES.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        const val TITLE = "Reconnect backoff"
        const val FIRST_LINE_OF_LATEST = "Here is where the reconnect work stands."
        const val LAST_LINE_OF_LATEST = "That is everything; the branch is ready for review."

        /** A long history whose newest reply is taller than the screen. */
        val MESSAGES = (1..12).flatMap { turn ->
            listOf(
                "user" to "Question $turn about the reconnect loop.",
                "assistant" to "Answer $turn. The connection actor retries on a timer, and this turn looked at part $turn of it.",
            )
        } + ("assistant" to (
            listOf(FIRST_LINE_OF_LATEST) +
                (1..14).map { "Step $it of the change touched another part of the actor and its tests, and each of them still passes." } +
                LAST_LINE_OF_LATEST
            ).joinToString("\n\n"))
    }
}
