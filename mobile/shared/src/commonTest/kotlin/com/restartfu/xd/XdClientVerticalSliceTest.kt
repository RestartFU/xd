package com.restartfu.xd

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.model.TranscriptKind
import com.restartfu.xd.net.FakeSocketFactory
import com.restartfu.xd.net.FakeSocket
import com.restartfu.xd.net.Link
import com.restartfu.xd.net.PairResult
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest

@OptIn(ExperimentalCoroutinesApi::class)
class XdClientVerticalSliceTest {
    @Test
    fun pairTreeChatSendAndStream() = runTest {
        val factory = FakeSocketFactory()
        val client = XdClient(factory, MemoryCredentialStore(), backgroundScope)
        val pairing = async {
            client.pair("daemon", 4001, "ABCD-EFGH", "Pixel")
        }
        runCurrent()

        val socket = factory.latest
        socket.connected(byteArrayOf(1, 2, 3))
        runCurrent()
        assertEquals("pair", socket.opAt(0))
        socket.receive("""{"ok":true,"token":"mobile-token"}""")
        runCurrent()
        assertIs<PairResult.Success>(pairing.await())
        assertIs<Link.Up>(client.link.value)

        assertEquals("tree", socket.opAt(1))
        socket.receive(
            """{"ok":true,"folders":[{"id":"folder","name":"Work"}],""" +
                """"chats":[{"id":"chat","folder":"folder","title":"Hello",""" +
                """"backend":"codex","working":false}]}""",
        )
        runCurrent()
        assertEquals("Hello", client.tree.value.chats.single().title)

        val session = client.openChat("chat")
        runCurrent()
        assertEquals("chat", socket.opAt(2))
        socket.receive(chatReply(working = false))
        runCurrent()
        assertEquals("messages", socket.opAt(3))
        socket.receive(messagesReply("""{"role":"user","content":"Earlier","at":1}"""))
        runCurrent()
        assertEquals("Earlier", session.state.value.messages.single().text)
        assertTrue(session.state.value.hasOlderMessages)

        socket.receive(
            """{"event":"commands","chat":"chat","backend":"codex","commands":["review","test"]}""",
        )
        runCurrent()
        assertEquals(listOf("review", "test"), session.state.value.commands)

        val cancelledOlder = async { session.loadOlder() }
        runCurrent()
        assertEquals("messages", socket.opAt(4))
        cancelledOlder.cancel()
        runCurrent()
        assertFalse(session.state.value.loadingOlder)
        socket.receive(messagesReply("""{"role":"user","content":"Ignored","at":0}"""))
        runCurrent()

        val loadingOlder = async { session.loadOlder() }
        runCurrent()
        assertEquals("messages", socket.opAt(5))
        assertTrue(socket.writes[5].decodeToString().contains(""""limit":300"""))
        socket.receive(
            messagesReply(
                """{"role":"user","content":"Oldest","at":0},""" +
                    """{"role":"user","content":"Earlier","at":1}""",
            ),
        )
        runCurrent()
        loadingOlder.await()
        assertEquals(listOf("Oldest", "Earlier"), session.state.value.messages.map { it.text })
        assertFalse(session.state.value.hasOlderMessages)

        val sending = async { session.send("Next") }
        runCurrent()
        assertEquals("send", socket.opAt(6))
        assertEquals("Next", session.state.value.pendingUser?.text)
        socket.receive("""{"ok":true}""")
        runCurrent()
        sending.await()

        socket.receive("""{"event":"turn-started","chat":"chat","label":"Codex"}""")
        runCurrent()
        assertTrue(session.state.value.working)
        assertTrue(client.tree.value.chats.single().working)
        assertEquals("chat", socket.opAt(7))
        socket.receive("""{"event":"text","chat":"chat","text":"Hel"}""")
        socket.receive(chatReply(working = true, segment = "Hel"))
        runCurrent()
        assertEquals("messages", socket.opAt(8))
        socket.receive("""{"event":"text","chat":"chat","text":"lo"}""")
        socket.receive(messagesReply("""{"role":"user","content":"Next","at":2}"""))
        runCurrent()
        assertEquals("Hello", session.state.value.liveSegment)

        socket.receive("""{"event":"turn-finished","chat":"chat","ok":true,"waiting":false}""")
        runCurrent()
        assertFalse(client.tree.value.chats.single().working)
        assertEquals("chat", socket.opAt(9))
        socket.receive(chatReply(working = false))
        runCurrent()
        assertEquals("messages", socket.opAt(10))
        socket.receive(
            messagesReply(
                """{"role":"user","content":"Next","at":2},""" +
                    """{"role":"assistant","content":"Done","at":3}""",
            ),
        )
        runCurrent()

        assertEquals(false, session.state.value.working)
        assertEquals(
            listOf(TranscriptKind.USER, TranscriptKind.ASSISTANT),
            session.state.value.messages.map { it.kind },
        )
        assertEquals("Done", session.state.value.messages.last().text)

        val cancelling = async { runCatching { session.cancel() } }
        runCurrent()
        assertEquals("cancel", socket.opAt(11))
        socket.receive("""{"ok":false,"error":"cancel rejected"}""")
        runCurrent()
        assertTrue(cancelling.await().isFailure)
        assertEquals("cancel rejected", session.state.value.error)

        client.forget()
        assertTrue(client.tree.value.chats.isEmpty())
        assertTrue(client.tree.value.folders.isEmpty())
        assertTrue(session.state.value.messages.isEmpty())
        assertTrue(session.state.value.title.isEmpty())
        val forgottenCall = runCatching { session.cancel() }
        assertTrue(forgottenCall.isFailure)
        assertTrue(forgottenCall.exceptionOrNull()?.message.orEmpty().contains("forgotten"))
        session.close()
    }

    @Test
    fun failedSendDuringReloadDoesNotRestoreOptimisticRow() = runTest {
        val factory = FakeSocketFactory()
        val client = XdClient(factory, MemoryCredentialStore(), backgroundScope)
        val pairing = async {
            client.pair("daemon", 4001, "ABCD-EFGH", "Pixel")
        }
        runCurrent()

        val socket = factory.latest
        socket.connected()
        runCurrent()
        socket.receive("""{"ok":true,"token":"mobile-token"}""")
        runCurrent()
        assertIs<PairResult.Success>(pairing.await())

        assertEquals("tree", socket.opAt(1))
        socket.receive("""{"ok":true,"folders":[],"chats":[]}""")
        runCurrent()

        val session = client.openChat("chat")
        runCurrent()
        assertEquals("chat", socket.opAt(2))

        val sending = async { runCatching { session.send("Rejected") } }
        runCurrent()
        assertEquals("send", socket.opAt(3))

        socket.receive(chatReply(working = false))
        runCurrent()
        assertEquals("messages", socket.opAt(4))

        socket.receive("""{"ok":false,"error":"send rejected"}""")
        runCurrent()
        assertTrue(sending.await().isFailure)

        socket.receive(messagesReply("", total = 0))
        runCurrent()

        assertNull(session.state.value.pendingUser)
        assertEquals("send rejected", session.state.value.error)
        assertTrue(session.state.value.messages.isEmpty())
        session.close()
    }

    @Test
    fun mutationFailureDuringReloadRemainsVisible() = runTest {
        val factory = FakeSocketFactory()
        val client = XdClient(factory, MemoryCredentialStore(), backgroundScope)
        val pairing = async {
            client.pair("daemon", 4001, "ABCD-EFGH", "Pixel")
        }
        runCurrent()

        val socket = factory.latest
        socket.connected()
        runCurrent()
        socket.receive("""{"ok":true,"token":"mobile-token"}""")
        runCurrent()
        assertIs<PairResult.Success>(pairing.await())
        socket.receive("""{"ok":true,"folders":[],"chats":[]}""")
        runCurrent()

        val session = client.openChat("chat")
        runCurrent()
        val cancelling = async { runCatching { session.cancel() } }
        runCurrent()
        socket.receive(chatReply(working = false))
        runCurrent()
        socket.receive("""{"ok":false,"error":"cancel rejected"}""")
        runCurrent()
        assertTrue(cancelling.await().isFailure)
        socket.receive(messagesReply("", total = 0))
        runCurrent()

        assertEquals("cancel rejected", session.state.value.error)
        session.close()
    }

    @Test
    fun malformedTreeReplyMakesProtocolFatal() = runTest {
        val factory = FakeSocketFactory()
        val client = XdClient(factory, MemoryCredentialStore(), backgroundScope)
        val pairing = async {
            client.pair("daemon", 4001, "ABCD-EFGH", "Pixel")
        }
        runCurrent()

        val socket = factory.latest
        socket.connected()
        runCurrent()
        socket.receive("""{"ok":true,"token":"mobile-token"}""")
        runCurrent()
        assertIs<PairResult.Success>(pairing.await())

        socket.receive("""{"ok":true,"folders":"invalid","chats":[]}""")
        runCurrent()
        runCurrent()

        assertEquals(
            com.restartfu.xd.net.FatalReason.PROTOCOL,
            assertIs<Link.Fatal>(client.link.value).reason,
        )
        assertTrue(socket.closed)
    }

    @Test
    fun missingNewChatIdMakesProtocolFatal() = runTest {
        val factory = FakeSocketFactory()
        val client = XdClient(factory, MemoryCredentialStore(), backgroundScope)
        val pairing = async {
            client.pair("daemon", 4001, "ABCD-EFGH", "Pixel")
        }
        runCurrent()

        val socket = factory.latest
        socket.connected()
        runCurrent()
        socket.receive("""{"ok":true,"token":"mobile-token"}""")
        runCurrent()
        assertIs<PairResult.Success>(pairing.await())
        socket.receive("""{"ok":true,"folders":[],"chats":[]}""")
        runCurrent()

        val creating = async {
            runCatching { client.createChat("folder", null) }
        }
        runCurrent()
        socket.receive("""{"ok":true}""")
        runCurrent()
        runCurrent()

        assertTrue(creating.await().isFailure)
        assertEquals(
            com.restartfu.xd.net.FatalReason.PROTOCOL,
            assertIs<Link.Fatal>(client.link.value).reason,
        )
        assertTrue(socket.closed)
    }

    @Test
    fun wireNewerLifecycleEventWinsOverInFlightTreeSnapshot() = runTest {
        val factory = FakeSocketFactory()
        val client = XdClient(factory, MemoryCredentialStore(), backgroundScope)
        val pairing = async {
            client.pair("daemon", 4001, "ABCD-EFGH", "Pixel")
        }
        runCurrent()
        val socket = factory.latest
        socket.connected()
        runCurrent()
        socket.receive("""{"ok":true,"token":"mobile-token"}""")
        runCurrent()
        assertIs<PairResult.Success>(pairing.await())

        socket.receive(treeReply(working = false))
        runCurrent()
        assertFalse(client.tree.value.chats.single().working)

        socket.receive("""{"event":"tree"}""")
        runCurrent()
        assertEquals("tree", socket.opAt(2))

        socket.receive(
            treeReply(working = false),
            """{"event":"turn-started","chat":"chat","label":"Codex"}""",
        )
        runCurrent()
        assertTrue(client.tree.value.chats.single().working)

        socket.receive("""{"event":"tree"}""")
        runCurrent()
        socket.receive(treeReply(working = true))
        val forgetting = async { client.forget() }
        runCurrent()
        forgetting.await()
        assertTrue(client.tree.value.chats.isEmpty())
    }

    @Test
    fun treeRefreshRequestsAreConflatedWhileRefreshRuns() = runTest {
        val factory = FakeSocketFactory()
        val client = XdClient(factory, MemoryCredentialStore(), backgroundScope)
        val pairing = async {
            client.pair("daemon", 4001, "ABCD-EFGH", "Pixel")
        }
        runCurrent()
        val socket = factory.latest
        socket.connected()
        runCurrent()
        socket.receive("""{"ok":true,"token":"mobile-token"}""")
        runCurrent()
        assertIs<PairResult.Success>(pairing.await())
        socket.receive(treeReply(working = false))
        runCurrent()

        socket.receive(
            """{"event":"tree"}""",
            """{"event":"tree"}""",
            """{"event":"tree"}""",
        )
        runCurrent()
        assertEquals(2, socket.countOps("tree"))

        socket.receive(treeReply(working = false))
        runCurrent()
        assertEquals(3, socket.countOps("tree"))
        socket.receive(treeReply(working = false))
        runCurrent()
        assertEquals(3, socket.countOps("tree"))
    }

    private fun FakeSocket.opAt(index: Int): String {
        val line = writes[index].decodeToString()
        return Regex(""""op":"([^"]+)"""").find(line)?.groupValues?.get(1)
            ?: error("No op in $line")
    }

    private fun FakeSocket.countOps(op: String): Int =
        writes.count { it.decodeToString().contains(""""op":"$op"""") }

    private fun chatReply(
        working: Boolean,
        segment: String? = null,
    ): String = buildString {
        append(
            """{"ok":true,"title":"Hello","backend":"codex","commands":[],""" +
                """"plan":false,"queue":[],"working":$working,"items":[],""" +
                """"new_worktree":false,"has_messages":true""",
        )
        if (segment != null) append(""","segment":"$segment"""")
        if (working) append(""","label":"Codex","working_for":1""")
        append("}")
    }

    private fun messagesReply(
        rows: String,
        total: Int = 2,
    ): String =
        """{"ok":true,"total_messages":$total,"last_message_id":2,"messages":[$rows]}"""

    private fun treeReply(working: Boolean): String =
        """{"ok":true,"folders":[{"id":"folder","name":"Work"}],""" +
            """"chats":[{"id":"chat","folder":"folder","title":"Hello",""" +
            """"backend":"codex","working":$working}]}"""
}
