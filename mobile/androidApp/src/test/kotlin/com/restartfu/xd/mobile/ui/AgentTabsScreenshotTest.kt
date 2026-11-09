package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.hasText
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.performClick
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.model.AgentRelay
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * The agents tray, an agent opened as its own tab, and a message relayed to
 * one, on a phone against a scripted host. With `-Pxd.shots=<dir>` it writes
 * PNG captures there for design review.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class AgentTabsScreenshotTest {
    @get:Rule
    val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val out = System.getProperty("xd.shots")?.let(::File)

    @Test fun trayCollapsed() = shoot("tray-collapsed-dark") {
        compose.onNodeWithText("2 agents and 1 command running").assertExists()
        // The relayed message shows only what the reader wrote, under the agent's name.
        compose.onNodeWithText("To Audit pairing retries").assertExists()
        compose.onNodeWithText(RELAY_TEXT).assertExists()
    }

    @Test fun trayOpen() = shoot("tray-open-dark") {
        compose.onNodeWithText("2 agents and 1 command running").performClick()
        settle()
        compose.onNode(trayRow("Trace reconnect retries")).assertExists()
        compose.onNode(trayRow("./gradlew :shared:testDebugUnitTest")).assertExists()
    }

    @Test fun trayOpenLight() = shoot("tray-open-light", MinimalThemePreset.LIGHT) {
        compose.onNodeWithText("2 agents and 1 command running").performClick()
        settle()
    }

    @Test fun agentView() = shoot("agent-view-dark") {
        compose.runOnUiThread { it.openAgent("toolu_audit") }
        settle()
        compose.onNodeWithText("Steps · 5").assertExists()
        compose.onNodeWithText("Report").assertExists()
        compose.onNodeWithText("Sent through the main chat").assertExists()
        compose.onNodeWithText("Queued in the main chat").assertExists()
        compose.onNodeWithText("Message this agent…").assertExists()
    }

    @Test fun agentViewLight() = shoot("agent-view-light", MinimalThemePreset.LIGHT) {
        compose.runOnUiThread { it.openAgent("toolu_audit") }
        settle()
    }

    /** A running Claude agent whose id has not been reported yet. */
    @Test fun runningAgentView() = shoot("agent-view-running-dark") {
        compose.runOnUiThread { it.openAgent("toolu_trace") }
        settle()
        compose.onNodeWithText("Messages open once Claude reports this agent's id.").assertExists()
        compose.onNodeWithText("Reading ConnectionActor.kt").assertExists()
    }

    /** A tray row opens its agent; the title menu lists it, and a chat there returns to the chat. */
    @Test fun openFromTray() = shoot("agent-from-tray-dark") { model ->
        compose.onNodeWithText("2 agents and 1 command running").performClick()
        settle()
        compose.onNode(trayRow("Review the backoff tests")).performClick()
        settle()
        assertEquals("019a2c4e-thread", model.viewingAgent.value)
        assertEquals(listOf("019a2c4e-thread"), model.agentTabs.value)
        // A Codex agent is addressed by its thread, so it takes messages at once.
        compose.onNodeWithText("Message this agent…").assertExists()
        compose.onNodeWithText("Review the backoff tests", useUnmergedTree = true).assertExists()
        compose.onNodeWithContentDescription("Close Review the backoff tests").assertDoesNotExist()
        compose.onNode(hasClickLabel("Switch chat")).performClick()
        settle()
        compose.onNodeWithContentDescription("Close Review the backoff tests").assertExists()
        compose.onNodeWithText("Release notes").performClick()
        settle()
        assertEquals(null, model.viewingAgent.value)
        assertEquals(listOf("019a2c4e-thread"), model.agentTabs.value)
    }

    private lateinit var chat: ChatViewModel

    private fun trayRow(task: String) = hasClickLabel("Open agent") and hasText(task)

    private fun hasClickLabel(label: String) = SemanticsMatcher("click label $label") {
        it.config.getOrElseNullable(SemanticsActions.OnClick) { null }?.label == label
    }

    private fun settle() = repeat(30) {
        compose.mainClock.advanceTimeBy(100)
        Thread.sleep(20)
    }

    private fun shoot(
        name: String,
        theme: MinimalThemePreset = MinimalThemePreset.DARK,
        act: (ChatViewModel) -> Unit,
    ) {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ ScriptedSocket() }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        chat = ChatViewModel(client, "chat")
        val chats = listOf(
            ChatSummary("chat", "folder", "Fix reconnect backoff", "claude", false, sessionId = "s"),
            ChatSummary("chat2", "folder", "Release notes", "codex", false, sessionId = "s"),
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
        check(chat.state.value.visibleItems.size >= MESSAGES.size)
        act(chat)
        // Drawn directly: the agents' pulsing dots never let Compose go idle,
        // which captureToImage waits for.
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
                val body = reply(op).replace("\n", "").removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"folders":[{"id":"folder","name":"xd"}],"chats":[""" +
                """{"id":"chat","folder":"folder","title":"Fix reconnect backoff","backend":"claude","working":false}]}"""
            "chat" -> """{"ok":true,"title":"Fix reconnect backoff","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[${JsonPrimitive(QUEUED)}],"working":false,"label":"Claude","items":[],"new_worktree":false,""" +
                """"has_messages":true,"model":"claude-opus-5-5","effort":"high"}"""
            "messages" -> """{"ok":true,"total_messages":${MESSAGES.size},"last_message_id":${MESSAGES.size},"messages":[""" +
                MESSAGES.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
                """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],"efforts":["high"]}]}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        const val RELAY_TEXT = "Also check what happens when the host key changes mid-retry."
        val QUEUED = AgentRelay.format("Audit pairing retries", "a22197aff550fdfd9", "And list every timeout you find.")
        val STARTED = System.currentTimeMillis() - 125_000

        fun record(json: String) = "subagent_run\n$json"

        val MESSAGES = listOf(
            "user" to "Split the reconnect work: trace the retries, review the backoff tests, and audit pairing.",
            "assistant" to "Starting agents for each part. I'll run the shared tests meanwhile.",
            "tool" to record(
                """{"key":"toolu_trace","provider":"Claude","agent":"Explore","description":"Trace reconnect retries",""" +
                    """"status":"running","background":true,"activity":"Reading ConnectionActor.kt","model":"claude-opus-5-5",""" +
                    """"prompt":"Find every place the client retries a connection and how long it waits.",""" +
                    """"steps":[{"tool":"Grep","text":"reconnect in shared/src"},{"tool":"Read","text":"net/ConnectionActor.kt"}],""" +
                    """"tool_uses":2,"started_at":$STARTED}""",
            ),
            "tool" to record(
                """{"key":"019a2c4e-thread","provider":"Codex","agent":"worker","description":"Review the backoff tests",""" +
                    """"status":"running","activity":"Running BackoffTest","tool_uses":4,"started_at":${STARTED + 40_000}}""",
            ),
            "tool" to record(
                """{"key":"bash_1","provider":"Claude","agent":"Shell","description":"",""" +
                    """"status":"running","background":true,"activity":"Running ./gradlew :shared:testDebugUnitTest",""" +
                    """"started_at":${STARTED + 90_000}}""",
            ),
            "tool" to record(
                """{"key":"toolu_audit","task_id":"a22197aff550fdfd9","provider":"Claude","agent":"general-purpose",""" +
                    """"description":"Audit pairing retries","status":"completed","model":"claude-opus-5-5",""" +
                    """"prompt":"Audit how pairing retries after a dropped connection. Report anything that retries without a limit.",""" +
                    """"steps":[{"tool":"Grep","text":"retry in credentials/"},{"tool":"Read","text":"PairingInvitation.kt"},""" +
                    """{"tool":"Read","text":"net/Backoff.kt"},{"tool":"Bash","text":"${'$'} ./gradlew :shared:test --tests *Pairing*"},""" +
                    """{"tool":"Read","text":"AndroidSocket.kt"}],"tool_uses":5,"tokens":18240,"duration_ms":94000,""" +
                    """"summary":"Pairing retries **three times** with a fixed 2 s wait, then gives up.\n\n- `PairingInvitation` has no backoff of its own.\n- A changed host key fails at once, without a retry."}""",
            ),
            "user" to AgentRelay.format("Audit pairing retries", "a22197aff550fdfd9", RELAY_TEXT),
            "assistant" to "Passed that on to the pairing audit.",
        )
    }
}
