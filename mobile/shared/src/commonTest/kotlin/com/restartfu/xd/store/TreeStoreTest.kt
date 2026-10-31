package com.restartfu.xd.store

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.credentials.testCredentials
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.net.FakeSocketFactory
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest

@OptIn(ExperimentalCoroutinesApi::class)
class TreeStoreTest {
    @Test
    fun sharedTerminalActivityBeforeSnapshotReachesEveryMember() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(testCredentials()), backgroundScope)
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receiveReadinessTreeReply()
        runCurrent()
        val store = TreeStore(actor)
        store.setChatTerminalWorking("session:workspace", true, sequence = 99)
        val refresh = async { store.refresh() }
        runCurrent()
        factory.latest.receive("""{"ok":true,"folders":[],"chats":[
            {"id":"one","folder":"folder","session":"workspace","title":"One","backend":"codex","working":false},
            {"id":"two","folder":"folder","session":"workspace","title":"Two","backend":"claude","working":false}],
            "sessions":[{"id":"workspace","folder":"folder","title":"Workspace","chats":["one","two"],"terminal_chat":"session:workspace"}],"_xd_request":2}""".replace("\n", ""))
        runCurrent()
        refresh.await()
        assertEquals(setOf("one", "two"), store.state.value.chats.map { it.id }.toSet())
        assertTrue(store.state.value.chats.all { it.terminalWorking })
        store.setChatTerminalWorking("session:workspace", false, sequence = 100)
        assertFalse(store.state.value.chats.any { it.terminalWorking })
        store.setChatTerminalWorking("one", true, sequence = 101)
        assertTrue(store.state.value.chats.all { it.terminalWorking }, "legacy terminal aliases reach the shared workspace")
    }

    @Test
    fun lifecycleEventCoveredByTreeSnapshotIsIgnored() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(
                testCredentials(),
            ),
            backgroundScope,
        )
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receiveReadinessTreeReply()
        runCurrent()
        runCurrent()
        runCurrent()

        val store = TreeStore(actor)
        val refresh = async { store.refresh() }
        runCurrent()
        factory.latest.receive(
            """{"ok":true,"folders":[],"chats":[{"id":"chat","folder":"folder",""" +
                """"title":"Hello","backend":"codex","working":false}],"_xd_request":2}""",
        )
        runCurrent()
        refresh.await()
        assertFalse(store.state.value.chats.single().working)

        store.setChatWorking("chat", working = true, sequence = 1)
        assertFalse(store.state.value.chats.single().working)

        store.setChatWorking("chat", working = true, sequence = 3)
        assertTrue(store.state.value.chats.single().working)

        store.setChatTerminalWorking("chat", working = true, sequence = 4)
        assertTrue(store.state.value.chats.single().terminalWorking)
    }
}
