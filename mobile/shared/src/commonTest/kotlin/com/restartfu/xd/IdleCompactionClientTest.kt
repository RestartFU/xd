package com.restartfu.xd

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.model.CompactBeforeSend
import com.restartfu.xd.model.IdleCompactionMode
import com.restartfu.xd.model.IdleCompactionSettings
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.net.FakeSocket
import com.restartfu.xd.net.FakeSocketFactory
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
class IdleCompactionClientTest {
    @Test
    fun theModeFollowsRepliesAndBroadcasts() = runTest {
        val (client, socket) = connected(backgroundScope)
        assertNull(client.idleCompactionSettings.value)

        val read = async { client.loadIdleCompactionSettings() }
        runCurrent()
        assertEquals("idle-compaction-settings", socket.lastOp())
        socket.receive("""{"ok":true,"mode":"onReturn"}""")
        runCurrent()
        assertEquals(IdleCompactionSettings(IdleCompactionMode.ON_RETURN), read.await())
        assertEquals(IdleCompactionSettings(IdleCompactionMode.ON_RETURN), client.idleCompactionSettings.value)

        // A broadcast carries another client's change.
        socket.receive("""{"event":"idle-compaction-settings","mode":"off"}""")
        runCurrent()
        assertEquals(IdleCompactionSettings(IdleCompactionMode.OFF), client.idleCompactionSettings.value)

        // A change whose reply does not name the mode keeps what it asked for.
        val set = async { client.setIdleCompactionSettings(IdleCompactionMode.BEFORE_CACHE_EXPIRES) }
        runCurrent()
        assertEquals("set-idle-compaction-settings", socket.lastOp())
        assertTrue(socket.lastLine().contains(""""mode":"beforeCacheExpires""""))
        socket.receive("""{"ok":true}""")
        runCurrent()
        assertEquals(IdleCompactionSettings(IdleCompactionMode.BEFORE_CACHE_EXPIRES), set.await())
        assertEquals(IdleCompactionSettings(IdleCompactionMode.BEFORE_CACHE_EXPIRES), client.idleCompactionSettings.value)
    }

    @Test
    fun aReplyOvertakenByABroadcastDoesNotUndoIt() = runTest {
        val (client, socket) = connected(backgroundScope)
        val set = async { client.setIdleCompactionSettings(IdleCompactionMode.OFF) }
        runCurrent()
        socket.receive("""{"event":"idle-compaction-settings","mode":"off"}""")
        socket.receive("""{"event":"idle-compaction-settings","mode":"onReturn"}""")
        runCurrent()
        socket.receive("""{"ok":true,"mode":"off"}""")
        runCurrent()
        set.await()
        assertEquals(IdleCompactionSettings(IdleCompactionMode.ON_RETURN), client.idleCompactionSettings.value)
    }

    @Test
    fun aBroadcastThatDoesNotSayIsReadAgainAndForgettingClearsTheMode() = runTest {
        val (client, socket) = connected(backgroundScope)
        socket.receive("""{"event":"idle-compaction-settings"}""")
        runCurrent()
        assertEquals("idle-compaction-settings", socket.lastOp())
        socket.receive("""{"ok":true,"mode":"beforeCacheExpires"}""")
        runCurrent()
        assertEquals(IdleCompactionSettings(IdleCompactionMode.BEFORE_CACHE_EXPIRES), client.idleCompactionSettings.value)
        client.forget()
        runCurrent()
        assertNull(client.idleCompactionSettings.value)
    }

    @Test
    fun aChatCarriesItsOfferAndASendCanCompactFirst() = runTest {
        val (client, socket) = connected(backgroundScope)
        val session = client.openChat("chat")
        client.setVisibleChat("chat")
        runCurrent()
        assertEquals("chat", socket.lastOp())
        socket.receive(
            """{"ok":true,"title":"Hello","backend":"claude","plan":false,"queue":[],"working":false,""" +
                """"context_used":251000,"context_window":1000000,""" +
                """"compact_before_send":{"tokens":251000,"after_ms":1763100000000},""" +
                """"items":[],"new_worktree":false,"has_messages":true}""",
        )
        runCurrent()
        socket.receive("""{"ok":true,"total_messages":0,"last_message_id":0,"messages":[]}""")
        runCurrent()
        assertEquals(CompactBeforeSend(251_000, 1_763_100_000_000), session.state.value.compactBeforeSend)

        val sending = async { session.send("and then", compactFirst = true) }
        runCurrent()
        assertEquals("send", socket.lastOp())
        assertTrue(socket.lastLine().contains(""""compact_first":true"""))
        socket.receive("""{"ok":true}""")
        runCurrent()
        sending.await()

        // The compaction starting means the chat is busy, not idle.
        socket.receive("""{"event":"turn-started","chat":"chat","turn_id":2,"label":"Claude"}""")
        runCurrent()
        assertNull(session.state.value.compactBeforeSend)
    }

    @Test
    fun aNewModeReadsTheOpenChatAgain() = runTest {
        val (client, socket) = connected(backgroundScope)
        client.openChat("chat")
        client.setVisibleChat("chat")
        runCurrent()
        socket.receive(
            """{"ok":true,"title":"Hello","backend":"claude","plan":false,"queue":[],"working":false,""" +
                """"items":[],"new_worktree":false,"has_messages":true}""",
        )
        runCurrent()
        socket.receive("""{"ok":true,"total_messages":0,"last_message_id":0,"messages":[]}""")
        runCurrent()
        val before = socket.sentOps().count { it == "chat" }
        socket.receive("""{"event":"idle-compaction-settings","mode":"off"}""")
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
        socket.receive("""{"ok":true,"folders":[],"chats":[]}""")
        runCurrent()
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
