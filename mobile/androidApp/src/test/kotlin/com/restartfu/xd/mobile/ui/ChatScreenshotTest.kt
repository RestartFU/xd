package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.test.assertDoesNotExist
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.ChatViewModel
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
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * Renders the phone chat screen against a scripted host.
 *
 * It checks that the screen composes with a real transcript. With
 * `-Pxd.shots=<dir>` it also writes PNG captures there for design review.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class ChatScreenshotTest {
    @get:Rule
    val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val out = System.getProperty("xd.shots")?.let(::File)

    @Test fun phoneDark() = shoot("phone-dark", MinimalThemePreset.DARK)

    @Test fun phoneLight() = shoot("phone-light", MinimalThemePreset.LIGHT)

    /** The height left with the keyboard open. */
    @Config(qualifiers = "w412dp-h560dp-420dpi")
    @Test fun phoneKeyboard() = shoot("phone-keyboard-dark", MinimalThemePreset.DARK)

    /** The turn has ended, and the agent still runs two background tasks. */
    @Test fun phoneWaitingOnBackgroundTasks() =
        shoot("phone-background-dark", MinimalThemePreset.DARK, backgroundTasks = 2)

    @Test fun phoneExpandedBackgroundTasks() =
        shoot("phone-background-expanded-dark", MinimalThemePreset.DARK, backgroundTasks = 2, expanded = true)

    @Test fun phoneExpandedBackgroundTasksLight() =
        shoot("phone-background-expanded-light", MinimalThemePreset.LIGHT, backgroundTasks = 2, expanded = true)

    /**
     * The background tasks are an agent and a command the transcript lists, so
     * the agents tray names them and the row that would repeat it stays away.
     */
    @Test fun phoneBackgroundTasksInTheTray() =
        shoot("phone-background-tray-dark", MinimalThemePreset.DARK, backgroundTasks = 2, agents = true)

    private fun shoot(
        name: String,
        theme: MinimalThemePreset,
        backgroundTasks: Int = 0,
        expanded: Boolean = false,
        agents: Boolean = false,
    ) {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val working = backgroundTasks == 0
        val messages = if (agents) MESSAGES + BACKGROUND_AGENTS else MESSAGES
        val client = XdClient({ ScriptedSocket(working, messages) }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, "chat")
        val chats = listOf(
            ChatSummary(
                "chat", "folder", "Fix reconnect backoff", "claude", working,
                sessionId = "s", backgroundTasks = backgroundTasks,
            ),
            ChatSummary("chat2", "folder", "Release notes", "codex", false, sessionId = "s"),
            ChatSummary("chat3", "folder", "Flaky pairing test", "codex", false, sessionId = "s"),
        )
        compose.mainClock.autoAdvance = false
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        ChatScreen(
                            model = chat,
                            goBack = {},
                            sessionTitle = "reconnect-backoff",
                            projectName = "xd",
                            backgroundTasks = backgroundTasks,
                            sessionChats = SessionChats(chats, "chat", {}, {}, {}, {}, {}),
                        )
                    }
                }
            }
        }
        repeat(60) {
            compose.mainClock.advanceTimeBy(100)
            Thread.sleep(30)
        }
        check(chat.state.value.visibleItems.size >= 6)
        if (agents) {
            compose.onNodeWithText("1 agent and 1 command running").assertIsDisplayed()
            compose.onNodeWithText("Waiting on 2 background tasks").assertDoesNotExist()
        }
        if (expanded) {
            compose.onNodeWithText("Waiting on 2 background tasks").performClick()
            repeat(30) {
                compose.mainClock.advanceTimeBy(100)
                Thread.sleep(30)
            }
            compose.mainClock.autoAdvance = true
            compose.onNodeWithText("/bin/sh: jq: command not found").performScrollTo().assertIsDisplayed()
            compose.mainClock.autoAdvance = false
        }
        // Drawn directly: the running turn's animation never lets Compose go
        // idle, which captureToImage waits for.
        org.robolectric.shadows.ShadowLooper.idleMainLooper()
        val view = compose.activity.window.decorView
        val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
        view.draw(android.graphics.Canvas(bitmap))
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
        scope.cancel()
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
    private class ScriptedSocket(
        private val working: Boolean,
        private val messages: List<Pair<String, String>>,
    ) : PlatformSocket {
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
                val body = reply(op).replace("\n", "").removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "background-task-output" -> """{"ok":true,"chat":"chat","tasks":[
                {"id":"tests","kind":"Shell","description":"Run tests","started_at":${System.currentTimeMillis() / 1000 - 125},
                 "lines":[{"text":"Running 42 tests"},{"text":"42 passed; waiting for integration tests"}]},
                {"id":"watch","kind":"Monitor","description":"Watch CI checks","started_at":${System.currentTimeMillis() / 1000 - 125},
                 "lines":[{"text":"Checking CI status…"},{"text":"/bin/sh: jq: command not found","error":true}]}]}"""
            "tree" -> """{"ok":true,"folders":[{"id":"folder","name":"xd"}],"chats":[""" +
                """{"id":"chat","folder":"folder","title":"Fix reconnect backoff","backend":"claude","working":$working}]}"""
            "chat" -> """{"ok":true,"title":"Fix reconnect backoff","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":$working,"label":"Claude","working_for":48,"items":[],"new_worktree":false,""" +
                """"has_messages":true,"model":"claude-opus-5-5","effort":"high"}"""
            "messages" -> """{"ok":true,"total_messages":${messages.size},"last_message_id":${messages.size},"messages":[""" +
                messages.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${kotlinx.serialization.json.JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
                """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],"efforts":["high"]}]}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        val STARTED = System.currentTimeMillis() - 17_000

        /** A reviewer agent and a CI watch the turn left running in the background. */
        val BACKGROUND_AGENTS = listOf(
            "tool" to "subagent_run\n" +
                """{"key":"toolu_review","provider":"Claude","agent":"general-purpose","description":"Review the reconnect change",""" +
                """"status":"running","background":true,"started_at":$STARTED}""",
            "tool" to "subagent_run\n" +
                """{"key":"toolu_ci","provider":"Claude","agent":"Shell","description":"Watch CI",""" +
                """"status":"running","background":true,"started_at":$STARTED}""",
            "assistant" to "I started a reviewer and a CI watch in the background. I'll report back when they finish.",
        )

        val MESSAGES = listOf(
            "user" to "The Android client hammers the host after a network drop. Make reconnect back off, and keep the chat usable while it retries.",
            "assistant" to "I'll start with the connection actor, since that's where retries are scheduled.",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt",
            "tool" to "Grep \"reconnect\" in shared/src",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/Link.kt",
            "assistant" to "Found it. `ConnectionActor` retries on a fixed 500 ms timer and never resets after a good connection:\n\n```kotlin\nwhile (!connected) {\n    delay(500)\n    connect()\n}\n```\n\nI'll replace that with capped exponential backoff with jitter, and reset the delay once the tree loads.",
            "tool" to "Edit shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt",
            "tool" to "$ ./gradlew --offline :shared:testDebugUnitTest",
            "assistant" to "Backoff now starts at 500 ms, doubles to a 30 s cap, and adds up to 20% jitter. `Link.Reconnecting` carries the next attempt time so the header can say when it will retry.\n\nAll 214 shared tests pass. Next I'll add a test that drops the socket three times and checks the delays.",
            "user" to "Good. Also show the retry countdown in the chat header.",
        )
    }
}

