package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.hasClickAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
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
import kotlinx.serialization.json.JsonPrimitive
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
 * Agents a chat's agent started with xd's start_agent tool: one row each in
 * the lead chat, a slim line for each report, and the way back from an
 * agent's chat to its lead, on a phone against a scripted host. With
 * `-Pxd.shots=<dir>` it writes PNG captures there for design review.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class AgentChatsScreenshotTest {
    @get:Rule
    val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val out = System.getProperty("xd.shots")?.let(::File)
    private val opened = mutableListOf<String>()

    @Test fun leadDark() = shoot("agents-lead-dark", LEAD, MESSAGES) {
        compose.onNodeWithText("BoatFly e2e test").assertExists()
        compose.onNodeWithText("Running").assertExists()
        compose.onNodeWithText("Waypoints e2e test finished").assertExists()
        // The report is a line, not the long message the lead's agent read.
        compose.onNodeWithText("Its reply:", substring = true).assertDoesNotExist()
        // The row is the button: its chevron has no name of its own to read out after the title.
        compose.onNodeWithContentDescription("Open DoubleClick e2e test").assertDoesNotExist()
        val row = compose.onNode(hasText("DoubleClick e2e test") and hasClickAction())
        assertEquals("Open chat", row.fetchSemanticsNode().config[SemanticsActions.OnClick].label)
        row.performClick()
        settle()
        assertEquals(listOf("double"), opened)
    }

    @Test fun leadLight() = shoot("agents-lead-light", LEAD, MESSAGES, MinimalThemePreset.LIGHT)

    /** Scrolled away from their rows, the tray above the composer still says what is at work. */
    @Test fun trayOpen() = shoot("agents-tray-dark", LEAD, MESSAGES, before = {
        compose.onNodeWithText("1 agent working · 1 needs you").performClick()
        settle()
    }) {
        // On the worker's row in the chat, and in the tray.
        assertEquals(2, compose.onAllNodesWithText("Needs you").fetchSemanticsNodes().size)
        assertEquals(2, compose.onAllNodesWithText("DoubleClick e2e test").fetchSemanticsNodes().size)
    }

    @Test fun workerDark() = shoot("agents-worker-dark", WORKER, WORKER_MESSAGES, startedBy = LEAD) {
        compose.onNodeWithText("Started by").performClick()
        settle()
        assertEquals(listOf("lead"), opened)
    }

    private fun settle() = repeat(30) {
        compose.mainClock.advanceTimeBy(100)
        Thread.sleep(20)
    }

    private fun shoot(
        name: String,
        chatSummary: ChatSummary,
        messages: List<Pair<String, String>>,
        theme: MinimalThemePreset = MinimalThemePreset.DARK,
        startedBy: ChatSummary? = null,
        before: () -> Unit = {},
        act: () -> Unit = {},
    ) {
        script = messages
        title = chatSummary.title
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ ScriptedSocket() }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, chatSummary.id)
        compose.mainClock.autoAdvance = false
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        ChatScreen(
                            model = chat,
                            goBack = {},
                            sessionTitle = chatSummary.title,
                            projectName = "lunar",
                            branch = chatSummary.branch,
                            agentChats = WORKERS.filter { it.parentId == chatSummary.id }.associateBy { it.id },
                            startedBy = startedBy,
                            openChat = { opened += it },
                        )
                    }
                }
            }
        }
        repeat(60) {
            compose.mainClock.advanceTimeBy(100)
            Thread.sleep(30)
        }
        check(chat.state.value.visibleItems.size >= messages.size) { "loaded ${chat.state.value.visibleItems.size} of ${messages.size}: ${chat.state.value.error}" }
        before()
        // Drawn directly: a working chat's indicator never lets Compose go
        // idle, which captureToImage waits for.
        org.robolectric.shadows.ShadowLooper.idleMainLooper()
        val view = compose.activity.window.decorView
        val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
        view.draw(android.graphics.Canvas(bitmap))
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
        act()
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
            "tree" -> """{"ok":true,"folders":[{"id":"lunar","name":"lunar"}],"chats":[]}"""
            "chat" -> """{"ok":true,"title":${JsonPrimitive(title)},"backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":false,"label":"Claude","items":[],"new_worktree":false,""" +
                """"has_messages":true,"model":"claude-opus-5-5","effort":"high"}"""
            "messages" -> """{"ok":true,"total_messages":${script.size},"last_message_id":${script.size},"messages":[""" +
                script.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
                """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],"efforts":["high"]}]}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        @Volatile var script: List<Pair<String, String>> = emptyList()
        @Volatile var title = ""

        val LEAD = ChatSummary("lead", "lunar", "Create e2e tests for 10 modules", "claude", false, branch = "main")
        val WORKER = ChatSummary(
            "boat", "lunar", "BoatFly e2e test", "claude", false,
            branch = "xd/boatfly-e2e-test", parentId = "lead",
        )
        val WORKERS = listOf(
            WORKER,
            ChatSummary("way", "lunar", "Waypoints e2e test", "claude", false, parentId = "lead", questionWaiting = true),
            ChatSummary("double", "lunar", "DoubleClick e2e test", "claude", true, parentId = "lead"),
        )

        fun card(chat: String, title: String, status: String, prompt: String, summary: String = "") =
            "subagent_run\n" + """{"key":"chat:$chat","chat":"$chat","provider":"Claude","agent":"$title",""" +
                """"description":"$title","status":"$status","background":true,"steps":[],"tool_uses":0,""" +
                """"model":"claude-opus-5-5","prompt":${JsonPrimitive(prompt)}""" +
                (if (summary.isEmpty()) "" else ""","summary":${JsonPrimitive(summary)}""") + "}"

        fun report(chat: String, title: String, reply: String) =
            "[xd] Agent \"$title\" ($chat) is done with its turn.\n\nIts reply:\n\n$reply\n\n" +
                "Answer it with message_agent if it needs more, or check on every agent with list_agents."

        const val BOAT = "Added tests/e2e/features/boatfly.feature: 6 scenarios covering the toggle, speed limits and the BDS and Geyser rows. All pass locally. Opened #1363."
        const val WAY = "Added tests/e2e/features/waypoints.feature: 9 scenarios for creating, renaming and sharing waypoints. Opened #1333."

        val MESSAGES = listOf(
            "user" to "Create e2e tests for the 10 modules that do not have one yet, one PR each.",
            "assistant" to "I'll give each module its own agent, so they work in parallel and each gets its own worktree and PR.",
            "tool" to card("boat", "BoatFly e2e test", "completed", "Write the e2e feature for the BoatFly module.", BOAT),
            "tool" to card("way", "Waypoints e2e test", "completed", "Write the e2e feature for the Waypoints module.", WAY),
            "tool" to card("double", "DoubleClick e2e test", "running", "Write the e2e feature for the DoubleClick module, covering every control."),
            "assistant" to "Three agents are on it: BoatFly, Waypoints and DoubleClick. I'll review each one as it reports back.",
            "user" to report("boat", "BoatFly e2e test", BOAT),
            "assistant" to "Reviewed BoatFly: the scenarios exercise the module's real effect. I'll merge it once CI is green.",
            "user" to report("way", "Waypoints e2e test", WAY),
            "assistant" to "Reviewed Waypoints: same bar, merging once CI is green.",
        )

        val WORKER_MESSAGES = listOf(
            "user" to "Write the e2e feature for the BoatFly module under tests/e2e/features, covering every control, mode and guard. Open a PR.",
            "assistant" to BOAT,
        )
    }
}
