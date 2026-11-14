package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.view.View
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsOff
import androidx.compose.ui.test.assertIsOn
import androidx.compose.ui.test.assertIsSelected
import androidx.compose.ui.test.hasAnyDescendant
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.isSelectable
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.MobileSettings
import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.boolean
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
 * Context compaction against a scripted host: its rows in the transcript,
 * the composer's offer to compact an idle chat before sending, and the idle
 * compaction setting. With `-Pxd.shots=<dir>` it writes PNG captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class CompactionScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    /** The rows, then the chip both ways, then a send that compacts first. */
    @Test fun anIdleChatOffersToCompactBeforeSending() {
        val socket = ScriptedSocket()
        chat(socket) { chat ->
            compose.runOnUiThread { chat.updateDraft(DRAFT) }
            settle()
            // Dividers, not tool rows, and none of them in the run of tool calls.
            check(shows("Context compacted 250K → 32K tokens"))
            check(shows("Context compaction failed · Not enough messages"))
            check(shows("Compacting context"))
            check(!shows("context_compaction"))
            check(!shows("tool calls"))
            check(chips().fetchSemanticsNodes().size == 1) { "No compact chip" }
            chip().assertIsOn()
            check(described(COMPACT_AND_SEND))
            // The model button gives its name room to the chip and keeps it as its label.
            check(described("Opus 5.5"))
            save("after-chat-compaction-rows-compact-chip-dark")

            chip().performClick()
            settle()
            chip().assertIsOff()
            check(!described(COMPACT_AND_SEND) && described("Send"))
            save("after-chat-keep-chip-dark")

            chip().performClick()
            settle()
            chip().assertIsOn()
            compose.onNode(hasContentDescription(COMPACT_AND_SEND)).performClick()
            val request = awaitSent(socket, "send")
            check(request["text"]?.jsonPrimitive?.content == DRAFT) { "Sent $request" }
            check(request["compact_first"]?.jsonPrimitive?.boolean == true) { "Sent $request" }
        }
    }

    @Test fun keepingTheHistorySendsPlainlyAndTheChipResets() {
        val socket = ScriptedSocket()
        chat(socket) { chat ->
            compose.runOnUiThread { chat.updateDraft(DRAFT) }
            settle()
            chip().performClick()
            settle()
            compose.onNode(hasContentDescription("Send")).performClick()
            val request = awaitSent(socket, "send")
            check("compact_first" !in request) { "Sent $request" }
            settle()
            check(!chat.keepHistory.value)
        }
    }

    @Test fun noOfferWhenTheHostCompactsOtherwise() {
        chat(ScriptedSocket(mode = "beforeCacheExpires")) { chat ->
            compose.runOnUiThread { chat.updateDraft(DRAFT) }
            settle()
            check(chips().fetchSemanticsNodes().isEmpty())
            check(!described(COMPACT_AND_SEND))
        }
    }

    @Test fun noOfferForACompactionTyped() {
        chat(ScriptedSocket()) { chat ->
            compose.runOnUiThread { chat.updateDraft("/compact keep the plan") }
            settle()
            check(chips().fetchSemanticsNodes().isEmpty())
        }
    }

    /** Not due yet when the chat loads, then due while it stays open, without a reload. */
    @Test fun theOfferAppearsOnceDue() {
        val socket = ScriptedSocket(compactAfterMs = System.currentTimeMillis() + 3_000)
        chat(socket) { _ ->
            check(chips().fetchSemanticsNodes().isEmpty())
            val chats = socket.sent("chat").size
            val deadline = System.currentTimeMillis() + 20_000
            while (chips().fetchSemanticsNodes().isEmpty() && System.currentTimeMillis() < deadline) {
                Thread.sleep(200)
                ShadowLooper.idleMainLooper(500, TimeUnit.MILLISECONDS)
                settle()
            }
            check(chips().fetchSemanticsNodes().size == 1) { "The chip never appeared" }
            check(socket.sent("chat").size == chats) { "The chat was read again" }
        }
    }

    @Test fun settingsOfferIdleCompaction() {
        val socket = ScriptedSocket()
        settings("after-settings-chats-dark", socket) {
            check(shows("Compact idle chats"))
            check(shows(IDLE_COMPACTION_DETAIL))
            val labels = listOf("When you return", "Before the cache expires (beta)", "Never")
            val tops = labels.map { label ->
                compose.onNode(hasText(label), useUnmergedTree = true).fetchSemanticsNode().boundsInRoot.top
            }
            check(tops == tops.sorted()) { "Out of order: $tops" }
            compose.onNode(isSelectable() and hasAnyDescendant(hasText("When you return")), useUnmergedTree = true).assertIsSelected()
        }
    }

    @Test fun choosingAModeTellsTheHost() {
        val socket = ScriptedSocket()
        settings(null, socket) {
            compose.onNode(isSelectable() and hasAnyDescendant(hasText("Never")), useUnmergedTree = true).performClick()
            val request = awaitSent(socket, "set-idle-compaction-settings")
            check(request["mode"]?.jsonPrimitive?.content == "off") { "Sent $request" }
            repeat(5) {
                ShadowLooper.idleMainLooper()
                settle()
            }
            compose.onNode(isSelectable() and hasAnyDescendant(hasText("Never")), useUnmergedTree = true).assertIsSelected()
        }
    }

    @Test fun olderHostsHaveNoSuchSetting() {
        settings(null, ScriptedSocket(compaction = false)) {
            check(!shows("Compact idle chats"))
            check(shows("Prompt shortcuts"))
        }
    }

    private fun chips() = compose.onAllNodes(hasContentDescription(compactChipDescription(251_000)))

    private fun chip() = compose.onNode(hasContentDescription(compactChipDescription(251_000)))

    private fun described(text: String) =
        compose.onAllNodes(hasContentDescription(text), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    private fun awaitSent(socket: ScriptedSocket, op: String): JsonObject {
        val deadline = System.currentTimeMillis() + 10_000
        while (socket.sent(op).isEmpty() && System.currentTimeMillis() < deadline) {
            ShadowLooper.idleMainLooper()
            compose.mainClock.advanceTimeBy(100)
            Thread.sleep(30)
        }
        return socket.sent(op).single()
    }

    private fun chat(socket: ScriptedSocket, steps: (ChatViewModel) -> Unit) {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ socket }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, "chat")
        val chats = listOf(
            ChatSummary("chat", "folder", "Fix reconnect backoff", "claude", false, sessionId = "s"),
            ChatSummary("chat2", "folder", "Release notes", "codex", false, sessionId = "s"),
        )
        val theme = MinimalThemePreset.DARK
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
        try {
            check(chat.state.value.transcriptLoaded)
            // The composer reads the host's mode once the chat offers to compact.
            val deadline = System.currentTimeMillis() + 10_000
            while (chat.idleCompaction.value == null && System.currentTimeMillis() < deadline) {
                ShadowLooper.idleMainLooper()
                settle()
            }
            settle()
            steps(chat)
        } finally {
            scope.cancel()
        }
    }

    private fun save(name: String) {
        ShadowLooper.idleMainLooper()
        repeat(5) {
            compose.mainClock.advanceTimeBy(50)
            Thread.sleep(15)
        }
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }

    private fun settings(name: String?, socket: ScriptedSocket, steps: () -> Unit) {
        val activity = compose.activity
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val model = MainViewModel(activity.application, XdClient({ socket }, NoCredentials, scope))
        val connected = runBlocking { model.client.connect(connection) }
        check(connected is ConnectResult.Success) { "Could not reach the scripted host: $connected" }
        val settings = MobileSettings(activity)
        val theme = MinimalThemePreset.DARK
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val link by model.client.link.collectAsStateWithLifecycle()
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) { MinimalMobileApp(model, link, settings) }
                }
            }
        }
        val deadline = System.currentTimeMillis() + 60_000
        while (model.client.tree.value.folders.isEmpty() && System.currentTimeMillis() < deadline) settle()
        settle()
        try {
            compose.onAllNodes(hasContentDescription("Settings"), useUnmergedTree = true).onFirst().performClick()
            repeat(5) {
                ShadowLooper.idleMainLooper()
                settle()
            }
            compose.onAllNodes(hasText("Chats")).onFirst().performClick()
            repeat(5) {
                ShadowLooper.idleMainLooper()
                settle()
            }
            compose.mainClock.autoAdvance = true
            compose.onNode(hasText("Prompt shortcuts")).performScrollTo()
            compose.mainClock.autoAdvance = false
            repeat(5) {
                ShadowLooper.idleMainLooper()
                settle()
            }
            steps()
        } finally {
            if (name != null) {
                out?.let { dir ->
                    dir.mkdirs()
                    File(dir, "$name.png").outputStream().use { capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
                }
            }
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

    /** Answers each request from a fixed script, echoing its request id, and keeps what was asked. */
    private class ScriptedSocket(
        /** When a send may compact first, in Unix ms; null offers none. */
        private val compactAfterMs: Long? = 1_000L,
        @Volatile var mode: String = "onReturn",
        /** Whether the host knows the idle compaction ops; older ones refuse them. */
        private val compaction: Boolean = true,
    ) : PlatformSocket {
        private var listener: PlatformSocketListener? = null
        private val requests = CopyOnWriteArrayList<JsonObject>()

        fun sent(op: String): List<JsonObject> = requests.filter { it["op"]?.jsonPrimitive?.content == op }

        fun push(line: String) {
            Thread { Thread.sleep(5); listener?.onBytes((line + "\n").encodeToByteArray()) }.start()
        }

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                requests += request
                val op = request["op"]?.jsonPrimitive?.content
                if (op == "set-idle-compaction-settings") mode = request["mode"]?.jsonPrimitive?.content ?: mode
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val body = reply(op)
                    .removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                push(body)
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"settlement":true,"chat_quotas":true,"folders":[{"id":"folder","name":"xd"}],""" +
                """"sessions":[],"chats":[{"id":"chat","folder":"folder","title":"Fix reconnect backoff",""" +
                """"backend":"claude","working":false}]}"""
            "chat-quota-settings" -> """{"ok":true,"percent":null}"""
            "idle-compaction-settings", "set-idle-compaction-settings" ->
                if (compaction) """{"ok":true,"mode":"$mode"}""" else """{"ok":false,"error":"Unknown op"}"""
            "chat-title-settings" -> """{"ok":true,"enabled":true,"models":{"codex":"gpt-6-luna","claude":"claude-haiku-5-5"}}"""
            "auto-settle-settings" -> """{"ok":true,"after_days":3,"on_merge":true,"keep_pull_requests":true}"""
            "chat" -> """{"ok":true,"title":"Fix reconnect backoff","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":false,"resumable":false,"items":[],"draft":"Now add a test for the countdown.",""" +
                """"new_worktree":false,"has_messages":true,"model":"claude-opus-5-5","effort":"high",""" +
                """"context_used":251000,"context_window":1000000""" +
                (compactAfterMs?.let { ""","compact_before_send":{"tokens":251000,"after_ms":$it}""" } ?: "") + "}"
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
        const val DRAFT = "Now add a test for the countdown."
        val MESSAGES = listOf(
            "user" to "The Android client hammers the host after a network drop. Make reconnect back off.",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt",
            "tool" to "context_compaction\ncompleted\nContext compacted 250K → 32K tokens",
            "tool" to "Edit shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt",
            "assistant" to "Backoff now starts at 500 ms and doubles to a 30 s cap.",
            "tool" to "context_compaction\nfailed\nContext compaction failed · Not enough messages",
            "user" to "/compact",
            "tool" to "context_compaction\nrunning\nCompacting context",
        )
    }
}
