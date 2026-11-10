package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.DiffViewModel
import com.restartfu.xd.model.ChatSummary
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
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The transcript's tool rows and the Changes pane against a scripted host.
 *
 * Checks that an opened row stays open when a new message arrives, and that
 * the Changes pane draws a patch. With `-Pxd.shots=<dir>` it writes captures.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class TranscriptRowsScreenshotTest {
    @get:Rule
    val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val out = System.getProperty("xd.shots")?.let(::File)

    @Test
    fun anOpenedToolRunStaysOpenWhenAMessageArrives() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val socket = ScriptedSocket()
        val client = XdClient({ socket }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, "chat")
        val chats = listOf(ChatSummary("chat", "folder", "Fix reconnect backoff", "claude", false, sessionId = "s"))
        compose.mainClock.autoAdvance = false
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(MinimalThemePreset.DARK)) {
                MaterialTheme(colorScheme = minimalColors(MinimalThemePreset.DARK), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        ChatScreen(
                            model = chat,
                            goBack = {},
                            sessionTitle = "reconnect-backoff",
                            projectName = "xd",
                            sessionChats = SessionChats(chats, "chat", {}, {}, {}, {}, {}),
                        )
                    }
                }
            }
        }
        settle()
        check(chat.state.value.messages.size == MESSAGES.size)
        compose.mainClock.autoAdvance = true
        compose.onNodeWithText("3 tool calls").performClick()
        compose.onNodeWithText("ConnectionActor.kt").performClick()
        compose.mainClock.autoAdvance = false
        settle()
        capture("chat-opened")
        assertTrue(compose.onAllNodesWithTextCount("Grep \"reconnect\" in shared/src") > 0)

        // Another device's message lands: the chat reads its messages again.
        socket.extra = true
        socket.push("""{"event":"changed","chat":"chat"}""")
        settle()
        check(chat.state.value.messages.size == MESSAGES.size + 1)
        capture("chat-after-new-message")
        assertTrue(
            "the opened run stays open",
            compose.onAllNodesWithTextCount("Grep \"reconnect\" in shared/src") > 0,
        )
        scope.cancel()
    }

    @Test
    fun theChangesPaneDrawsAPatch() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ ScriptedSocket() }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, "chat")
        val diff = DiffViewModel { chat.session }
        compose.mainClock.autoAdvance = false
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(MinimalThemePreset.DARK)) {
                MaterialTheme(colorScheme = minimalColors(MinimalThemePreset.DARK), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) { DiffPaneContent(diff) }
                }
            }
        }
        settle()
        capture("changes")
        assertTrue(compose.onAllNodesWithTextCount("shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt") > 0)
        scope.cancel()
    }

    private fun androidx.compose.ui.test.junit4.AndroidComposeTestRule<*, *>.onAllNodesWithTextCount(text: String): Int =
        onAllNodes(androidx.compose.ui.test.hasText(text, substring = true)).fetchSemanticsNodes().size

    private fun settle() {
        repeat(40) {
            compose.mainClock.advanceTimeBy(100)
            Thread.sleep(30)
            org.robolectric.shadows.ShadowLooper.idleMainLooper()
        }
    }

    private fun capture(name: String) {
        org.robolectric.shadows.ShadowLooper.idleMainLooper()
        val view = compose.activity.window.decorView
        val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
        view.draw(android.graphics.Canvas(bitmap))
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
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
        @Volatile private var listener: PlatformSocketListener? = null

        /** Whether the newer message has been stored. */
        @Volatile var extra = false

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        fun push(line: String) {
            listener?.onBytes((line + "\n").encodeToByteArray())
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val op = request["op"]?.jsonPrimitive?.content
                val body = reply(op).replace("\n", "").removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"folders":[{"id":"folder","name":"xd"}],"chats":[""" +
                """{"id":"chat","folder":"folder","title":"Fix reconnect backoff","backend":"claude","working":false}]}"""
            "chat" -> """{"ok":true,"title":"Fix reconnect backoff","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":false,"items":[],"new_worktree":false,""" +
                """"has_messages":true,"model":"claude-opus-5-5","effort":"high"}"""
            "messages" -> {
                val rows = if (extra) MESSAGES + NEWER else MESSAGES
                """{"ok":true,"total_messages":${rows.size},"last_message_id":${rows.size},"messages":[""" +
                    rows.mapIndexed { i, (role, text) ->
                        """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                    }.joinToString(",") + "]}"
            }
            "git-status" -> """{"ok":true,"branch":"reconnect-backoff","base":"master","unstaged":2,"clean":false}"""
            "diff-read" -> """{"ok":true,"output":${JsonPrimitive(PATCH)}}"""
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
                """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],"efforts":["high"]}]}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        val PATCH = """
            diff --git a/shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt b/shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt
            index 3f1c2a0..9b7d4e1 100644
            --- a/shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt
            +++ b/shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt
            @@ -40,9 +40,14 @@ internal class ConnectionActor(
                 private suspend fun reconnect() {
            -        while (!connected) {
            -            delay(500)
            -            connect()
            -        }
            +        var wait = INITIAL_BACKOFF_MILLIS
            +        while (!connected) {
            +            delay(wait + Random.nextLong(wait / 5 + 1))
            +            if (connect()) break
            +            // Doubles up to a cap, so a host that is down is not hammered.
            +            wait = (wait * 2).coerceAtMost(MAX_BACKOFF_MILLIS)
            +        }
                 }
            diff --git a/shared/src/commonMain/kotlin/com/restartfu/xd/net/Link.kt b/shared/src/commonMain/kotlin/com/restartfu/xd/net/Link.kt
            index 1a2b3c4..5d6e7f8 100644
            --- a/shared/src/commonMain/kotlin/com/restartfu/xd/net/Link.kt
            +++ b/shared/src/commonMain/kotlin/com/restartfu/xd/net/Link.kt
            @@ -12,5 +12,6 @@ public sealed interface Link {
                 public data class Up(val remoteName: String) : Link
            -    public data object Reconnecting : Link
            +    /** Retrying; [nextAttemptMillis] says when, for a countdown. */
            +    public data class Reconnecting(val nextAttemptMillis: Long) : Link
                 public data object Down : Link
        """.trimIndent() + "\n"

        val MESSAGES = listOf(
            "user" to "The Android client hammers the host after a network drop. Make reconnect back off.",
            "assistant" to "I'll start with the connection actor, since that's where retries are scheduled.",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt",
            "tool" to "Grep \"reconnect\" in shared/src",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/Link.kt",
            "tool" to "file_change\n" + PATCH.substringBefore("diff --git a/shared/src/commonMain/kotlin/com/restartfu/xd/net/Link.kt"),
            "assistant" to "Backoff now starts at 500 ms, doubles to a 30 s cap, and adds up to 20% jitter.",
        )

        val NEWER = "user" to "Good. Also show the retry countdown in the chat header."
    }
}
