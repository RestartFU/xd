package com.restartfu.xd.store

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.credentials.testCredentials
import com.restartfu.xd.model.LinkedPullRequest
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
    fun linkedPullRequestsReachEachChat() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(testCredentials()), backgroundScope)
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receiveReadinessTreeReply()
        runCurrent()
        val store = TreeStore(actor)
        val refresh = async { store.refresh() }
        runCurrent()
        factory.latest.receive(
            """{"ok":true,"linked_pull_requests":true,"folders":[],"chats":[""" +
                """{"id":"one","folder":"folder","title":"One","backend":"codex","working":false,"pull_requests":[""" +
                """{"url":"https://github.com/o/r/pull/7","state":"merged"},{"url":"https://github.com/o/r/pull/9","state":""}]},""" +
                """{"id":"two","folder":"folder","title":"Two","backend":"codex","working":false}],"_xd_request":2}""",
        )
        runCurrent()
        refresh.await()
        assertTrue(store.state.value.linkedPullRequests)
        assertEquals(
            listOf(
                LinkedPullRequest("https://github.com/o/r/pull/7", "merged"),
                LinkedPullRequest("https://github.com/o/r/pull/9", null),
            ),
            store.state.value.chats.first { it.id == "one" }.pullRequests,
        )
        assertEquals(emptyList(), store.state.value.chats.first { it.id == "two" }.pullRequests)
    }

    @Test
    fun treeCarriesPauseSupportAndPausedChats() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(testCredentials()), backgroundScope)
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receiveReadinessTreeReply()
        runCurrent()
        val store = TreeStore(actor)
        val refresh = async { store.refresh() }
        runCurrent()
        factory.latest.receive(
            """{"ok":true,"pause_all":true,"folders":[],"chats":[""" +
                """{"id":"one","folder":"f","title":"One","backend":"codex","working":false,"paused":true},""" +
                """{"id":"two","folder":"f","title":"Two","backend":"claude","working":false}],"_xd_request":2}""",
        )
        runCurrent()
        refresh.await()
        assertTrue(store.state.value.pauseAll)
        assertEquals(listOf(true, false), store.state.value.chats.map { it.paused })
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

    @Test
    fun backgroundTaskCountsFollowEventsNewerThanTheSnapshot() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(testCredentials()), backgroundScope)
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receiveReadinessTreeReply()
        runCurrent()
        val store = TreeStore(actor)
        store.setChatBackgroundTasks("chat", 1, sequence = 99)
        val refresh = async { store.refresh() }
        runCurrent()
        factory.latest.receive(
            """{"ok":true,"folders":[],"chats":[{"id":"chat","folder":"folder",""" +
                """"title":"Hello","backend":"claude","working":false,"background_tasks":2}],"_xd_request":2}""",
        )
        runCurrent()
        refresh.await()
        assertEquals(1, store.state.value.chats.single().backgroundTasks, "the event is newer than the snapshot")
        assertEquals(1, store.state.value.chats.single().waitingOn)

        store.setChatBackgroundTasks("chat", 3, sequence = 98)
        assertEquals(1, store.state.value.chats.single().backgroundTasks, "an older event is ignored")
        store.setChatBackgroundTasks("chat", 0, sequence = 100)
        assertEquals(0, store.state.value.chats.single().waitingOn)
    }
}
