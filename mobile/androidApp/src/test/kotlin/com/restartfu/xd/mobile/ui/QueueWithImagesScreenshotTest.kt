package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
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
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.Base64
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
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
 * Renders the chat screen before and after queueing a draft with an image
 * while a turn runs, against a host scripted to queue the way the real one
 * does: a send keeps its images as references, the queue op keeps text only.
 *
 * With `-Pxd.shots=<dir>` it writes `queue-image-composer.png` and
 * `queue-image-queued.png` there.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-420dpi")
class QueueWithImagesScreenshotTest {
    @get:Rule
    val compose = createAndroidComposeRule<androidx.activity.ComponentActivity>()

    private val out = System.getProperty("xd.shots")?.let(::File)

    @Test
    fun queueingADraftWithAnImage() {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val socket = ScriptedSocket(Base64.getEncoder().encodeToString(png()))
        val client = XdClient({ socket }, NoCredentials, scope)
        runBlocking { client.connect(connection) }
        val chat = ChatViewModel(client, "chat")
        val chats = listOf(ChatSummary("chat", "folder", "Tidy the settings page", "claude", true, sessionId = "s"))
        compose.mainClock.autoAdvance = false
        compose.setContent {
            CompositionLocalProvider(LocalXdSignals provides xdSignals(MinimalThemePreset.DARK)) {
                MaterialTheme(colorScheme = minimalColors(MinimalThemePreset.DARK), typography = XdTypography) {
                    Surface(Modifier.fillMaxSize()) {
                        ChatScreen(
                            model = chat,
                            goBack = {},
                            sessionTitle = "settings-spacing",
                            projectName = "xd",
                            sessionChats = SessionChats(chats, "chat", {}, {}, {}, {}, {}),
                        )
                    }
                }
            }
        }
        settle({ "working=${chat.state.value.working} attachments=${chat.attachments.value.size}" }) { chat.state.value.working && chat.attachments.value.size == 1 }
        capture("queue-image-composer")

        // What the keyboard's send key does while a turn runs.
        chat.enqueue()
        settle({ "queue=${chat.state.value.queue} sending=${chat.sending.value} attachments=${chat.attachments.value.size} error=${chat.state.value.error} ops=${socket.ops}" }) { chat.state.value.queue.isNotEmpty() && !chat.sending.value }
        capture("queue-image-queued")
        scope.cancel()
    }

    private fun settle(describe: () -> String, condition: () -> Boolean) {
        repeat(150) {
            compose.mainClock.advanceTimeBy(100)
            ShadowLooper.idleMainLooper()
            Thread.sleep(20)
            if (condition()) {
                repeat(10) {
                    compose.mainClock.advanceTimeBy(100)
                    Thread.sleep(20)
                }
                return
            }
        }
        error("Timed out waiting for the chat screen: ${describe()}")
    }

    /** Drawn directly: the running turn's animation never lets Compose go idle. */
    private fun capture(name: String) {
        ShadowLooper.idleMainLooper()
        val view = compose.activity.window.decorView
        val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
        view.draw(android.graphics.Canvas(bitmap))
        out?.let { dir ->
            dir.mkdirs()
            File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        }
    }

    private fun png(): ByteArray {
        val bitmap = Bitmap.createBitmap(320, 200, Bitmap.Config.ARGB_8888)
        val canvas = android.graphics.Canvas(bitmap)
        canvas.drawColor(0xff3b6fd8.toInt())
        val paint = android.graphics.Paint().apply { color = 0xffffffff.toInt(); textSize = 28f; isAntiAlias = true }
        canvas.drawText("settings.png", 82f, 110f, paint)
        return ByteArrayOutputStream().use { out ->
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
            out.toByteArray()
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

    /** A host with a running turn and a synced draft holding one image. */
    private class ScriptedSocket(private val image: String) : PlatformSocket {
        private var listener: PlatformSocketListener? = null
        private var queueRevision = 0
        val ops = java.util.concurrent.CopyOnWriteArrayList<String>()

        override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
            this.listener = listener
            Thread { listener.onConnected() }.start()
        }

        override fun send(bytes: ByteArray) {
            bytes.decodeToString().lines().filter { it.isNotBlank() }.forEach { line ->
                val request = Json.parseToJsonElement(line).jsonObject
                val id = request["_xd_request"]?.jsonPrimitive?.longOrNull
                val op = request["op"]?.jsonPrimitive?.content
                ops += op.orEmpty()
                val text = request["text"]?.jsonPrimitive?.content.orEmpty()
                // The host queues a send that arrives mid-turn with each image
                // saved and referenced on its own line; the queue op has no images.
                val queued = when (op) {
                    "send" -> (request["attachments"] as? JsonArray).orEmpty().indices.fold(text) { prompt, at ->
                        "$prompt\n[image: /home/dev/.local/share/xd/remote-pasted/paste-${at + 1}.png]"
                    }
                    "queue" -> text
                    else -> null
                }
                val body = reply(op).replace("\n", "").removeSuffix("}") +
                    (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread {
                    Thread.sleep(5)
                    listener?.onBytes((body + "\n").encodeToByteArray())
                    if (queued != null) {
                        queueRevision++
                        val event = """{"event":"queued","chat":"chat","queue":[${JsonPrimitive(queued)}],""" +
                            """"queue_revision":$queueRevision}"""
                        listener?.onBytes((event + "\n").encodeToByteArray())
                    }
                }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"folders":[{"id":"folder","name":"xd"}],"chats":[""" +
                """{"id":"chat","folder":"folder","title":"Tidy the settings page","backend":"claude","working":true}]}"""
            "chat" -> """{"ok":true,"title":"Tidy the settings page","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":true,"label":"Claude","working_for":48,"items":[],"new_worktree":false,""" +
                """"has_messages":true,"model":"claude-opus-5-5","effort":"high",""" +
                """"draft":"Match the spacing in this screenshot","draft_revision":1,""" +
                """"draft_attachments":[{"name":"settings.png","mime":"image/png","data":"$image"}]}"""
            "messages" -> """{"ok":true,"total_messages":2,"last_message_id":2,"messages":[""" +
                """{"id":1,"role":"user","content":"Tidy the spacing on the settings page","at":1762500000},""" +
                """{"id":2,"role":"assistant","content":"Looking at the settings layout now.","at":1762500001}]}"""
            "send" -> """{"ok":true,"queued":true}"""
            "agent-catalog" -> """{"ok":true,"backends":[{"id":"claude","name":"Claude Code",""" +
                """"default_model":"claude-opus-5-5","models":[{"id":"claude-opus-5-5","name":"Opus 5.5"}],"efforts":["high"]}]}"""
            else -> """{"ok":true}"""
        }
    }
}
