package com.restartfu.xd.store

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.net.FakeSocketFactory
import com.restartfu.xd.net.Link
import com.restartfu.xd.net.SequencedEvent
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

@OptIn(ExperimentalCoroutinesApi::class)
class ChatSessionCoreTest {
    @Test
    fun speechFlowEmitsOnlyCompleteAssistantBlocksAndResetsForTools() = runTest {
        val core = ChatSessionCore("chat", testActor(backgroundScope), backgroundScope) { 10_000L }
        val spoken = async { core.speech.take(2).toList() }
        runCurrent()

        core.onEvent(textEvent(1, "<speak>hel"))
        core.onEvent(textEvent(2, "lo</spe"))
        core.onEvent(textEvent(3, "ak>"))
        core.onEvent(toolEvent(4, "Read"))
        core.onEvent(textEvent(5, "<speak>after tool</speak>"))

        assertEquals(listOf("hello", "after tool"), spoken.await())
    }

    @Test
    fun eventCoveredByCompletedSnapshotIsDiscarded() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(
                StoredCredentials(
                    host = "host",
                    port = 4001,
                    token = "token",
                    certificateDer = byteArrayOf(1, 2, 3),
                ),
            ),
            backgroundScope,
        )
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receive("""{"ok":true,"device":"Pixel","version":1}""")
        runCurrent()
        runCurrent()
        assertIs<Link.Up>(actor.link.value)

        val core = ChatSessionCore("chat", actor, backgroundScope) { 10_000L }
        val reload = async { core.reload() }
        runCurrent()
        factory.latest.receive(
            """{"ok":true,"title":"Hello","backend":"codex","commands":[],""" +
                """"plan":false,"queue":[],"working":true,"items":[],""" +
                """"segment":"Hel","new_worktree":false,"has_messages":true}""",
        )
        runCurrent()
        factory.latest.receive(
            """{"ok":true,"total_messages":0,"last_message_id":0,"messages":[]}""",
        )
        runCurrent()
        reload.await()
        assertNull(core.state.value.error)
        assertEquals("Hel", core.state.value.liveSegment)

        core.onEvent(
            SequencedEvent(
                sequence = 2,
                value = buildJsonObject {
                    put("event", "text")
                    put("chat", "chat")
                    put("text", "Hel")
                },
            ),
        )

        assertEquals("Hel", core.state.value.liveSegment)
    }

    @Test
    fun reloadRequestsAreConflatedWhileReloadRuns() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(
                StoredCredentials(
                    host = "host",
                    port = 4001,
                    token = "token",
                    certificateDer = byteArrayOf(1, 2, 3),
                ),
            ),
            backgroundScope,
        )
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receive("""{"ok":true,"device":"Pixel","version":1}""")
        runCurrent()
        runCurrent()

        val core = ChatSessionCore("chat", actor, backgroundScope) { 10_000L }
        core.requestReload()
        runCurrent()
        core.requestReload()
        core.requestReload()
        core.requestReload()
        runCurrent()
        assertEquals(
            1,
            factory.latest.countOps("chat"),
            factory.latest.decodedWrites(),
        )

        factory.latest.receive(chatReply())
        runCurrent()
        factory.latest.receive(messagesReply())
        runCurrent()
        assertEquals(
            2,
            factory.latest.countOps("chat"),
            factory.latest.decodedWrites(),
        )
    }

    @Test
    fun chatScopedEventWithoutChatIdIsIgnored() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(
                StoredCredentials(
                    host = "host",
                    port = 4001,
                    token = "token",
                    certificateDer = byteArrayOf(1, 2, 3),
                ),
            ),
            backgroundScope,
        )
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receive("""{"ok":true,"device":"Pixel","version":1}""")
        runCurrent()
        runCurrent()

        val core = ChatSessionCore("chat", actor, backgroundScope) { 10_000L }
        core.onEvent(
            SequencedEvent(
                sequence = 2,
                value = buildJsonObject {
                    put("event", "text")
                    put("text", "wrong chat")
                },
            ),
        )

        assertEquals("", core.state.value.liveSegment)
    }

    @Test
    fun malformedQueuedEventDoesNotReplaceAuthoritativeQueue() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(
                StoredCredentials(
                    host = "host",
                    port = 4001,
                    token = "token",
                    certificateDer = byteArrayOf(1, 2, 3),
                ),
            ),
            backgroundScope,
        )
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receive("""{"ok":true,"device":"Pixel","version":1}""")
        runCurrent()
        runCurrent()

        val core = ChatSessionCore("chat", actor, backgroundScope) { 10_000L }
        core.onEvent(
            SequencedEvent(
                sequence = 2,
                value = buildJsonObject {
                    put("event", "queued")
                    put("chat", "chat")
                    put("queue", JsonArray(listOf(JsonPrimitive("safe"))))
                },
            ),
        )
        core.onEvent(
            SequencedEvent(
                sequence = 3,
                value = buildJsonObject {
                    put("event", "queued")
                    put("chat", "chat")
                    put(
                        "queue",
                        JsonArray(
                            listOf(
                                JsonPrimitive("wrong"),
                                JsonPrimitive(1),
                            ),
                        ),
                    )
                },
            ),
        )

        assertEquals(listOf("safe"), core.state.value.queue)
    }

    private fun textEvent(sequence: Long, text: String): SequencedEvent =
        SequencedEvent(
            sequence = sequence,
            value = buildJsonObject {
                put("event", "text")
                put("chat", "chat")
                put("text", text)
            },
        )

    private fun toolEvent(sequence: Long, text: String): SequencedEvent =
        SequencedEvent(
            sequence = sequence,
            value = buildJsonObject {
                put("event", "tool")
                put("chat", "chat")
                put("text", text)
            },
        )

    private fun testActor(scope: CoroutineScope): ConnectionActor = ConnectionActor(
        FakeSocketFactory(),
        MemoryCredentialStore(
            StoredCredentials(
                host = "host",
                port = 4001,
                token = "token",
                certificateDer = byteArrayOf(1, 2, 3),
            ),
        ),
        scope,
    )

    private fun com.restartfu.xd.net.FakeSocket.countOps(op: String): Int =
        writes.count { it.decodeToString().contains(""""op":"$op"""") }

    private fun com.restartfu.xd.net.FakeSocket.decodedWrites(): String =
        writes.joinToString(separator = " | ") { it.decodeToString().trim() }

    private fun chatReply(): String =
        """{"ok":true,"title":"Hello","backend":"codex","commands":[],"plan":false,"queue":[],"working":false,"items":[],"new_worktree":false,"has_messages":true}"""

    private fun messagesReply(): String =
        """{"ok":true,"total_messages":0,"last_message_id":0,"messages":[]}"""
}
