package com.restartfu.xd.store

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.net.FakeSocketFactory
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest

@OptIn(ExperimentalCoroutinesApi::class)
class TreeStoreTest {
    @Test
    fun lifecycleEventCoveredByTreeSnapshotIsIgnored() = runTest {
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

        val store = TreeStore(actor)
        val refresh = async { store.refresh() }
        runCurrent()
        factory.latest.receive(
            """{"ok":true,"folders":[],"chats":[{"id":"chat","folder":"folder",""" +
                """"title":"Hello","backend":"codex","working":false}]}""",
        )
        runCurrent()
        refresh.await()
        assertFalse(store.state.value.chats.single().working)

        store.setChatWorking("chat", working = true, sequence = 2)
        assertFalse(store.state.value.chats.single().working)

        store.setChatWorking("chat", working = true, sequence = 3)
        assertTrue(store.state.value.chats.single().working)

        store.setChatTerminalWorking("chat", working = true, sequence = 4)
        assertTrue(store.state.value.chats.single().terminalWorking)
    }
}
