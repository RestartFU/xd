package com.restartfu.xd.mobile

import android.graphics.Bitmap
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.net.PlatformSocket
import com.restartfu.xd.net.PlatformSocketListener
import java.io.ByteArrayOutputStream
import java.util.Base64
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper

/**
 * A message queued behind a running turn keeps its images.
 *
 * The queue op carries text alone, so a draft with images has to go as a
 * send, which the host queues with the images saved and referenced.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35])
class QueueWithImagesTest {
    @Test
    fun queueingADraftWithAnImageSendsTheImage() {
        val png = png()
        val socket = ScriptedSocket(Base64.getEncoder().encodeToString(png))
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        try {
            val client = XdClient({ socket }, NoCredentials, scope)
            runBlocking { client.connect(connection) }
            val chat = ChatViewModel(client, "chat")
            awaitMain { chat.state.value.working && chat.attachments.value.size == 1 }

            chat.enqueue()
            awaitMain { socket.requests.any { it.op == "send" || it.op == "queue" } && !chat.sending.value }

            val ops = socket.requests.map { it.op }
            assertTrue("a draft with images must not use the text-only queue op: $ops", "queue" !in ops)
            val send = socket.requests.single { it.op == "send" }
            assertEquals("look at this", send["text"]?.jsonPrimitive?.content)
            val attachments = send["attachments"]!!.jsonArray
            assertEquals(1, attachments.size)
            assertEquals(
                Base64.getEncoder().encodeToString(png),
                attachments[0].jsonObject["data"]?.jsonPrimitive?.content,
            )
            assertTrue("sent images leave the composer", chat.attachments.value.isEmpty())
        } finally {
            scope.cancel()
        }
    }

    private val JsonObject.op get() = this["op"]?.jsonPrimitive?.content

    private fun awaitMain(condition: () -> Boolean) {
        repeat(250) {
            ShadowLooper.idleMainLooper()
            if (condition()) return
            Thread.sleep(20)
        }
        error("Timed out waiting for the view model")
    }

    private fun png(): ByteArray {
        val bitmap = Bitmap.createBitmap(4, 4, Bitmap.Config.ARGB_8888)
        bitmap.eraseColor(0xff3366cc.toInt())
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
        val requests = CopyOnWriteArrayList<JsonObject>()

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
                val body = reply(op).removeSuffix("}") +
                    (if (id != null) ""","_xd_request":$id}""" else "}")
                Thread { Thread.sleep(5); listener?.onBytes((body + "\n").encodeToByteArray()) }.start()
            }
        }

        override fun close() = Unit

        private fun reply(op: String?): String = when (op) {
            "tree" -> """{"ok":true,"folders":[{"id":"folder","name":"xd"}],"chats":[""" +
                """{"id":"chat","folder":"folder","title":"Chat","backend":"claude","working":true}]}"""
            "chat" -> """{"ok":true,"title":"Chat","backend":"claude","commands":[],"plan":false,""" +
                """"queue":[],"working":true,"items":[],"new_worktree":false,"has_messages":true,""" +
                """"draft":"look at this","draft_revision":1,""" +
                """"draft_attachments":[{"name":"image.png","mime":"image/png","data":"$image"}]}"""
            "messages" -> """{"ok":true,"total_messages":0,"last_message_id":0,"messages":[]}"""
            "send" -> """{"ok":true,"queued":true}"""
            else -> """{"ok":true}"""
        }
    }
}
