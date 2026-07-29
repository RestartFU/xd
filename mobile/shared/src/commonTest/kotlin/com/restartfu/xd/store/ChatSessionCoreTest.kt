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
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

@OptIn(ExperimentalCoroutinesApi::class)
class ChatSessionCoreTest {
    @Test
    fun eventCoveredByCompletedSnapshotIsDiscarded() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(
                StoredCredentials(
                    host = "daemon",
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
}
