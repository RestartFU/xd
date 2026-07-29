package com.restartfu.xd

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.model.TranscriptKind
import com.restartfu.xd.net.FakeSocketFactory
import com.restartfu.xd.net.Link
import com.restartfu.xd.net.PairResult
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
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

        val sending = async { session.send("Next") }
        runCurrent()
        assertEquals("send", socket.opAt(4))
        assertEquals("Next", session.state.value.pendingUser?.text)
        socket.receive("""{"ok":true}""")
        runCurrent()
        sending.await()

        socket.receive("""{"event":"turn-started","chat":"chat","label":"Codex"}""")
        runCurrent()
        assertTrue(session.state.value.working)
        assertEquals("chat", socket.opAt(5))
        socket.receive(chatReply(working = true, segment = "Hel"))
        runCurrent()
        assertEquals("messages", socket.opAt(6))
        socket.receive("""{"event":"text","chat":"chat","text":"lo"}""")
        socket.receive(messagesReply("""{"role":"user","content":"Next","at":2}"""))
        runCurrent()
        assertEquals("Hello", session.state.value.liveSegment)

        socket.receive("""{"event":"turn-finished","chat":"chat","ok":true,"waiting":false}""")
        runCurrent()
        assertEquals("chat", socket.opAt(7))
        socket.receive(chatReply(working = false))
        runCurrent()
        assertEquals("messages", socket.opAt(8))
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
        session.close()
    }

    private fun com.restartfu.xd.net.FakeSocket.opAt(index: Int): String {
        val line = writes[index].decodeToString()
        return Regex(""""op":"([^"]+)"""").find(line)?.groupValues?.get(1)
            ?: error("No op in $line")
    }

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

    private fun messagesReply(rows: String): String =
        """{"ok":true,"total_messages":2,"last_message_id":2,"messages":[$rows]}"""
}
