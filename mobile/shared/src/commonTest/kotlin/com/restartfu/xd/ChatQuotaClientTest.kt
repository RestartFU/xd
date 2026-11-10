package com.restartfu.xd

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.model.ChatQuota
import com.restartfu.xd.model.ChatQuotaSettings
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.net.FakeSocket
import com.restartfu.xd.net.FakeSocketFactory
import com.restartfu.xd.protocol.Ops
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest

@OptIn(ExperimentalCoroutinesApi::class)
class ChatQuotaClientTest {
    @Test
    fun theHostDefaultFollowsRepliesAndBroadcasts() = runTest {
        val (client, socket) = connected(backgroundScope)
        assertNull(client.chatQuotaSettings.value)

        val read = async { client.loadChatQuotaSettings() }
        runCurrent()
        assertEquals("chat-quota-settings", socket.lastOp())
        socket.receive("""{"ok":true,"percent":null}""")
        runCurrent()
        assertEquals(ChatQuotaSettings(null), read.await())
        assertEquals(ChatQuotaSettings(null), client.chatQuotaSettings.value)

        // A broadcast carries the new default.
        socket.receive("""{"event":"chat-quota-settings","percent":25}""")
        runCurrent()
        assertEquals(ChatQuotaSettings(25), client.chatQuotaSettings.value)

        // A change whose reply does not name the default keeps what it asked for.
        val set = async { client.setChatQuotaSettings(10) }
        runCurrent()
        assertTrue(socket.lastLine().contains(""""percent":10"""))
        socket.receive("""{"ok":true}""")
        runCurrent()
        assertEquals(ChatQuotaSettings(10), set.await())
        assertEquals(ChatQuotaSettings(10), client.chatQuotaSettings.value)
    }

    @Test
    fun aReplyOvertakenByABroadcastDoesNotUndoIt() = runTest {
        val (client, socket) = connected(backgroundScope)
        val set = async { client.setChatQuotaSettings(10) }
        runCurrent()
        // Another client's later change is broadcast before this reply lands.
        socket.receive("""{"event":"chat-quota-settings","percent":10}""")
        socket.receive("""{"event":"chat-quota-settings","percent":30}""")
        runCurrent()
        socket.receive("""{"ok":true,"percent":10}""")
        runCurrent()
        set.await()
        assertEquals(ChatQuotaSettings(30), client.chatQuotaSettings.value)
    }

    @Test
    fun aBroadcastThatDoesNotSayIsReadAgainAndForgettingClearsTheDefault() = runTest {
        val (client, socket) = connected(backgroundScope)
        socket.receive("""{"event":"chat-quota-settings"}""")
        runCurrent()
        assertEquals("chat-quota-settings", socket.lastOp())
        socket.receive("""{"ok":true,"percent":5}""")
        runCurrent()
        assertEquals(ChatQuotaSettings(5), client.chatQuotaSettings.value)
        client.forget()
        runCurrent()
        assertNull(client.chatQuotaSettings.value)
    }

    @Test
    fun aChatStoppedAtItsQuotaCarriesItAndAskingForANewOneSendsIt() = runTest {
        val (client, socket) = connected(backgroundScope)
        val session = client.openChat("chat")
        runCurrent()
        assertEquals("chat", socket.lastOp())
        socket.receive(
            """{"ok":true,"title":"Hello","backend":"codex","plan":false,"queue":["later"],"working":false,""" +
                """"resumable":true,"quota_reached":true,"quota":{"percent":10,"used":10.2,"own":false},""" +
                """"items":[],"new_worktree":false,"has_messages":true}""",
        )
        runCurrent()
        socket.receive("""{"ok":true,"total_messages":0,"last_message_id":0,"messages":[]}""")
        runCurrent()
        val state = session.state.value
        assertTrue(state.quotaReached)
        assertTrue(state.resumable)
        assertEquals(ChatQuota(10, 10.2, own = false), state.quota)

        val asking = async { session.setQuota(Ops.ChatQuotaChoice.Percent(20)) }
        runCurrent()
        assertEquals("set-chat-quota", socket.lastOp())
        assertTrue(socket.lastLine().contains(""""percent":20"""))
        assertTrue(socket.lastLine().contains(""""chat":"chat""""))
        socket.receive("""{"ok":true,"resumed":true}""")
        runCurrent()
        assertTrue(asking.await())

        // The continued turn starting clears the mark before any reload.
        socket.receive("""{"event":"turn-started","chat":"chat","turn_id":2,"label":"Codex"}""")
        runCurrent()
        assertFalse(session.state.value.quotaReached)
    }

    @Test
    fun aNewDefaultReadsTheOpenChatAgain() = runTest {
        val (client, socket) = connected(backgroundScope)
        client.openChat("chat")
        runCurrent()
        socket.receive(
            """{"ok":true,"title":"Hello","backend":"codex","plan":false,"queue":[],"working":false,""" +
                """"resumable":false,"quota":{"percent":10,"used":1.0,"own":false},""" +
                """"items":[],"new_worktree":false,"has_messages":true}""",
        )
        runCurrent()
        socket.receive("""{"ok":true,"total_messages":0,"last_message_id":0,"messages":[]}""")
        runCurrent()
        val before = socket.sentOps().count { it == "chat" }
        socket.receive("""{"event":"chat-quota-settings","percent":null}""")
        runCurrent()
        assertTrue(socket.sentOps().count { it == "chat" } > before)
    }

    private suspend fun TestScope.connected(scope: CoroutineScope): Pair<XdClient, FakeSocket> {
        val factory = FakeSocketFactory()
        val client = XdClient(factory, MemoryCredentialStore(), scope)
        val connecting = async { client.connect(connection) }
        runCurrent()
        val socket = factory.latest
        socket.connected()
        runCurrent()
        socket.receiveReadinessTreeReply()
        runCurrent()
        assertIs<ConnectResult.Success>(connecting.await())
        socket.receive("""{"ok":true,"chat_quotas":true,"folders":[],"chats":[]}""")
        runCurrent()
        assertTrue(client.tree.value.chatQuotas)
        return client to socket
    }

    private fun FakeSocket.lastLine(): String = writes.last().decodeToString()

    private fun FakeSocket.sentOps(): List<String> = writes.mapNotNull {
        Regex(""""op":"([^"]+)"""").find(it.decodeToString())?.groupValues?.get(1)
    }

    private fun FakeSocket.lastOp(): String =
        Regex(""""op":"([^"]+)"""").find(lastLine())?.groupValues?.get(1) ?: error("No op in ${lastLine()}")

    private val connection = SshConnection(
        host = "host",
        port = 22,
        username = "alice",
        authentication = SshAuthentication.Password("secret"),
        hostKey = SshHostKey("ssh-ed25519", byteArrayOf(1, 2, 3), "SHA256:test"),
    )
}
