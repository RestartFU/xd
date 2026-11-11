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
import androidx.compose.ui.test.hasContentDescription
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
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
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
 * Chat quotas against a scripted host: the host-wide setting in Settings,
 * and what a chat stopped at its quota docks in its composer in place of
 * its resume button. With `-Pxd.shots=<dir>` it writes PNG captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class ChatQuotaScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    @Test fun settingsOfferTheQuotaOnHostsThatHaveIt() = settings("chat-quota-settings-dark", chatQuotas = true) {
        check(shows("Chat quota"))
        check(shows("Limit each chat to 10% of its account's 5-hour usage limit. A chat that reaches it stops and asks before going on."))
        check(shows("Quota per chat"))
    }

    @Test fun settingsLeaveItOutOnOlderHosts() = settings("chat-quota-settings-older-host-dark", chatQuotas = false) {
        check(!shows("Chat quota"))
    }

    /** Stopped at its quota with a message queued: the quota ask replaces the play button. */
    @Test fun aChatStoppedAtItsQuotaOffersANewOne() {
        val socket = ScriptedSocket(chatQuotas = true, quotaReached = true)
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
            check(chat.state.value.quotaReached)
            ShadowLooper.idleMainLooper()
            val view = compose.activity.window.decorView
            val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
            view.draw(Canvas(bitmap))
            out?.let { dir ->
                dir.mkdirs()
                File(dir, "chat-quota-reached-dark.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
            }
            // Docked in the composer, with no second "waiting" status on it.
            check(shows("This chat reached its quota") && !shows("Waiting on you"))
            check(shows("10%") && shows("used of the 5-hour limit, past its 10% quota"))
            check(shows("New quota") && shows("Continue without a limit"))
            // What is sent meanwhile waits, which the composer says.
            check(shows("Messages you send wait until it goes on…"))
            // Prefilled with twice the last quota.
            check(compose.onAllNodes(hasText("20"), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty())
            // The play button would ask the host to resume, which it refuses here.
            check(compose.onAllNodes(hasContentDescription("Continue where it left off")).fetchSemanticsNodes().isEmpty())

            compose.onNode(hasText("Continue") and !hasText("Continue without a limit")).performClick()
            val deadline = System.currentTimeMillis() + 10_000
            while (socket.sent("set-chat-quota").isEmpty() && System.currentTimeMillis() < deadline) {
                compose.mainClock.advanceTimeBy(100)
                Thread.sleep(30)
            }
            val request = socket.sent("set-chat-quota").single()
            check(request["chat"]?.jsonPrimitive?.content == "chat")
            check(request["percent"]?.jsonPrimitive?.content == "20") { "Asked for $request" }
        } finally {
            scope.cancel()
        }
    }

    @Test fun continuingWithoutALimitSendsNoPercent() {
        val socket = ScriptedSocket(chatQuotas = true, quotaReached = true)
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ socket }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, "chat")
        compose.mainClock.autoAdvance = false
        compose.setContent {
            MaterialTheme(colorScheme = minimalColors(MinimalThemePreset.DARK), typography = XdTypography) {
                Surface(Modifier.fillMaxSize()) { ChatScreen(model = chat, goBack = {}, sessionTitle = "s", projectName = "xd") }
            }
        }
        repeat(60) {
            compose.mainClock.advanceTimeBy(100)
            Thread.sleep(30)
        }
        try {
            compose.onNode(hasText("Continue without a limit")).performClick()
            val deadline = System.currentTimeMillis() + 10_000
            while (socket.sent("set-chat-quota").isEmpty() && System.currentTimeMillis() < deadline) {
                compose.mainClock.advanceTimeBy(100)
                Thread.sleep(30)
            }
            val request = socket.sent("set-chat-quota").single()
            check(request["percent"] is JsonNull && "inherit" !in request) { "Asked for $request" }
        } finally {
            scope.cancel()
        }
    }

    private fun settings(name: String, chatQuotas: Boolean, steps: () -> Unit) {
        val activity = compose.activity
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val model = MainViewModel(activity.application, XdClient({ ScriptedSocket(chatQuotas) }, NoCredentials, scope))
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
            // The host's settings sit at the end of a long list.
            compose.mainClock.autoAdvance = true
            compose.onNode(hasText("Assistant accounts")).performScrollTo()
            compose.mainClock.autoAdvance = false
            repeat(5) {
                ShadowLooper.idleMainLooper()
                settle()
            }
            steps()
        } finally {
            out?.let { dir ->
                dir.mkdirs()
                File(dir, "$name.png").outputStream().use { capture().compress(Bitmap.CompressFormat.PNG, 100, it) }
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
        private val chatQuotas: Boolean,
        private val quotaReached: Boolean = false,
    ) : PlatformSocket {
        private var listener: PlatformSocketListener? = null
        private val requests = CopyOnWriteArrayList<JsonObject>()

        fun sent(op: String): List<JsonObject> = requests.filter { it["op"]?.jsonPrimitive?.content == op }

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                requests += request
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val body = reply(request["op"]?.jsonPrimitive?.content)
                    .removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"settlement":true,"chat_quotas":$chatQuotas,"folders":[{"id":"folder","name":"xd"}],""" +
                """"sessions":[],"chats":[{"id":"chat","folder":"folder","title":"Fix reconnect backoff",""" +
                """"backend":"claude","working":false,"quota_reached":$quotaReached}]}"""
            "chat-quota-settings" -> """{"ok":true,"percent":10}"""
            "chat-title-settings" -> """{"ok":true,"enabled":true,"models":{"codex":"gpt-6-luna","claude":"claude-haiku-5-5"}}"""
            "auto-settle-settings" -> """{"ok":true,"after_days":3,"on_merge":true,"keep_pull_requests":true}"""
            "chat" -> """{"ok":true,"title":"Fix reconnect backoff","backend":"claude","commands":[],"plan":false,""" +
                """"queue":["Then add a test for the countdown."],"working":false,"resumable":true,"items":[],""" +
                """"new_worktree":false,"has_messages":true,"model":"claude-opus-5-5","effort":"high"""" +
                (if (quotaReached) ""","quota_reached":true,"quota":{"percent":10,"used":10.2,"own":false}""" else "") + "}"
            "messages" -> """{"ok":true,"total_messages":${MESSAGES.size},"last_message_id":${MESSAGES.size},"messages":[""" +
                MESSAGES.mapIndexed { i, (role, text) ->
                    """{"id":${i + 1},"role":"$role","content":${JsonPrimitive(text)},"at":${1762500000 + i}}"""
                }.joinToString(",") + "]}"
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
                """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],"efforts":["high"]}]}"""
            "set-chat-quota" -> """{"ok":true,"resumed":true}"""
            else -> """{"ok":true}"""
        }
    }

    private companion object {
        val MESSAGES = listOf(
            "user" to "The Android client hammers the host after a network drop. Make reconnect back off.",
            "assistant" to "I'll start with the connection actor, since that's where retries are scheduled.",
            "tool" to "Read shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt",
            "tool" to "Edit shared/src/commonMain/kotlin/com/restartfu/xd/net/ConnectionActor.kt",
            "assistant" to "Backoff now starts at 500 ms and doubles to a 30 s cap. Next I'll run the shared tests.",
        )
    }
}
