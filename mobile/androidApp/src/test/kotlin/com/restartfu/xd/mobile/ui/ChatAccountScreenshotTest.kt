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
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.performClick
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.mobile.ChatViewModel
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
import kotlinx.serialization.json.JsonObject
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
 * Choosing the account a chat runs on, in its settings, against a scripted
 * host with two Claude accounts. With `-Pxd.shots=<dir>` it writes PNG
 * captures there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class ChatAccountScreenshotTest {
    private val out = System.getProperty("xd.shots")?.let(::File)

    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    @Test fun aChatIsSetToAnAccountInItsSettings() = settings(ScriptedSocket(chatAccounts = true)) { socket ->
        check(shows("Account") && shows(AUTOMATIC_ACCOUNT))
        check(shows("dana@example.com") && shows("dana.work@example.com · Out of its 5-hour limit", substring = true))
        check(selected(AUTOMATIC_ACCOUNT)) { "The host chooses until an account is picked" }
        capture("chat-settings-account-dark")
        // Listed for the choice alone, so the host does not check each account again.
        check(socket.sent("agent-auth").all { it["check"]?.jsonPrimitive?.content == "false" })

        compose.onNode(hasText("dana@example.com")).performClick()
        waitFor { socket.sent("set-option").isNotEmpty() }
        val request = socket.sent("set-option").single()
        check(request["option"]?.jsonPrimitive?.content == "account" && request["value"]?.jsonPrimitive?.content == "default") { "Asked for $request" }
        waitFor { selected("dana@example.com") }
        capture("chat-settings-account-chosen-dark")

        compose.onNode(hasText(AUTOMATIC_ACCOUNT)).performClick()
        waitFor { socket.sent("set-option").size == 2 }
        check(socket.sent("set-option").last()["value"]?.jsonPrimitive?.content == "") { "The host chooses again" }
    }

    @Test fun olderHostsAndLoneAccountsOfferNoChoice() {
        settings(ScriptedSocket(chatAccounts = false)) { socket ->
            check(!shows("Account") && !shows(AUTOMATIC_ACCOUNT))
            // An older host would check every account for a list nothing shows.
            check(socket.sent("agent-auth").isEmpty())
            capture("chat-settings-older-host-dark")
        }
    }

    @Test fun anAssistantWithOneAccountOffersNoChoice() =
        settings(ScriptedSocket(chatAccounts = true, accounts = 1)) { socket ->
            waitFor { socket.sent("agent-auth").isNotEmpty() }
            settle()
            check(!shows("Account") && !shows(AUTOMATIC_ACCOUNT))
        }

    private fun settings(socket: ScriptedSocket, steps: (ScriptedSocket) -> Unit) {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val client = XdClient({ socket }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, "chat")
        val theme = MinimalThemePreset.DARK
        compose.mainClock.autoAdvance = false
        compose.setContent {
            val state by chat.state.collectAsStateWithLifecycle()
            CompositionLocalProvider(LocalXdSignals provides xdSignals(theme)) {
                MaterialTheme(colorScheme = minimalColors(theme), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) { ChatOptionsDialog(chat, state) {} }
                }
            }
        }
        try {
            waitFor { client.tree.value.folders.isNotEmpty() && chat.state.value.backend == "claude" }
            repeat(4) { settle() }
            steps(socket)
        } finally {
            scope.cancel()
        }
    }

    private fun settle() {
        repeat(10) {
            Thread.sleep(15)
            compose.mainClock.advanceTimeBy(50)
        }
        ShadowLooper.idleMainLooper()
    }

    private fun waitFor(condition: () -> Boolean) {
        val deadline = System.currentTimeMillis() + 30_000
        while (!condition() && System.currentTimeMillis() < deadline) settle()
        check(condition()) { "Timed out" }
        settle()
    }

    private fun shows(text: String, substring: Boolean = false) =
        compose.onAllNodes(hasText(text, substring = substring), useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty()

    /** Whether the choice labelled [text] is the one picked. */
    private fun selected(text: String) =
        compose.onAllNodes(hasText(text)).fetchSemanticsNodes()
            .any { it.config.getOrNull(SemanticsProperties.Selected) == true }

    /** The activity with the dialog over it drawn where it sits. */
    private fun capture(name: String) {
        val dir = out ?: return
        ShadowLooper.idleMainLooper()
        val global = Class.forName("android.view.WindowManagerGlobal")
        val instance = global.getMethod("getInstance").invoke(null)
        val field = global.getDeclaredField("mViews").apply { isAccessible = true }
        @Suppress("UNCHECKED_CAST")
        val all = (field.get(instance) as List<View>).toList()
        val screen = compose.activity.window.decorView
        val bitmap = Bitmap.createBitmap(screen.width, screen.height, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        screen.draw(canvas)
        for (view in all.drop(all.indexOf(screen).coerceAtLeast(0))) {
            if (view === screen || view.width == 0) continue
            val dialog = (view.layoutParams as? WindowManager.LayoutParams)?.type ==
                WindowManager.LayoutParams.TYPE_APPLICATION
            if (dialog) canvas.drawColor(Color.argb(82, 0, 0, 0))
            canvas.save()
            canvas.translate((screen.width - view.width) / 2f, (screen.height - view.height) / 2f)
            view.draw(canvas)
            canvas.restore()
        }
        dir.mkdirs()
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

    /**
     * Answers each request from a fixed script, echoing its request id, and
     * keeps what was asked. It remembers the account the chat was set to, and
     * says the chat changed, as the host does.
     */
    private class ScriptedSocket(
        private val chatAccounts: Boolean,
        private val accounts: Int = 2,
    ) : PlatformSocket {
        private var listener: PlatformSocketListener? = null
        private val requests = CopyOnWriteArrayList<JsonObject>()

        @Volatile private var account: String? = null

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
                val op = request["op"]?.jsonPrimitive?.content
                var event: String? = null
                if (op == "set-option" && request["option"]?.jsonPrimitive?.content == "account") {
                    account = request["value"]?.jsonPrimitive?.content?.takeIf(String::isNotEmpty)
                    event = """{"event":"changed","chat":"chat"}"""
                }
                val body = reply(op).removeSuffix("}") + (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread {
                    Thread.sleep(5)
                    listener?.onBytes((body + "\n").encodeToByteArray())
                    event?.let { listener?.onBytes((it + "\n").encodeToByteArray()) }
                }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"settlement":true,"chat_accounts":$chatAccounts,"folders":[{"id":"folder","name":"xd"}],""" +
                """"sessions":[],"chats":[{"id":"chat","folder":"folder","title":"Fix reconnect backoff",""" +
                """"backend":"claude","working":false}]}"""
            "chat" -> """{"ok":true,"title":"Fix reconnect backoff","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":false,"items":[],"new_worktree":false,"has_messages":true,""" +
                """"model":"claude-opus-5-5","effort":"high","access":"edit"""" +
                (account?.let { ""","account":"$it"""" } ?: "") + "}"
            "messages" -> """{"ok":true,"total_messages":0,"last_message_id":0,"messages":[]}"""
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
                """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],""" +
                """"efforts":["low","medium","high"]}]}"""
            "agent-auth" -> {
                val limited = System.currentTimeMillis() + 2 * 3_600_000L
                val listed = listOf(
                    """{"id":"default","default":true,"label":"dana@example.com","plan":"Max","state":"signed-in","in_use":true}""",
                    """{"id":"b7c8d9e0f1a2","label":"dana.work@example.com","plan":"Max","state":"signed-in",""" +
                        """"limited_until_ms":$limited,"limited_reason":"5-hour limit"}""",
                ).take(accounts).joinToString(",")
                """{"ok":true,"multiple_accounts":true,"providers":[{"provider":"claude","display_name":"Claude Code",""" +
                    """"state":"signed-in","auto_switch":true,"accounts":[$listed]}]}"""
            }
            else -> """{"ok":true}"""
        }
    }
}
