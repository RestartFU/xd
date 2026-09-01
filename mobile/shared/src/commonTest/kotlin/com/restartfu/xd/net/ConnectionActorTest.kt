package com.restartfu.xd.net

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.jsonPrimitive

@OptIn(ExperimentalCoroutinesApi::class)
class ConnectionActorTest {
    @Test
    fun credentialLoadFailureStillMakesTheActorReadyForANewConnection() = runTest {
        var saved: StoredCredentials? = null
        val store = object : CredentialStore {
            override suspend fun load(): StoredCredentials? = error("keystore unavailable")

            override suspend fun save(credentials: StoredCredentials) {
                saved = credentials
            }

            override suspend fun clear() {
                saved = null
            }
        }
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, store, backgroundScope)
        runCurrent()

        assertTrue(actor.credentialsReady.value)
        assertFalse(actor.hasCredentials.value)
        assertEquals(Link.Idle, actor.link.value)

        val request = connection(
            hostKey = SshHostKey("ssh-ed25519", byteArrayOf(1), "SHA256:new"),
        )
        val result = async { actor.connect(request) }
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receive(treeReply())
        runCurrent()

        assertEquals(ConnectResult.Success("alice@host"), result.await())
        assertEquals(request, saved?.connection)
    }

    @Test
    fun storedTransportIsNotReadyUntilHostProtocolReplies() = runTest {
        val hostKey = byteArrayOf(9, 8, 7)
        val factory = FakeSocketFactory()
        val store = MemoryCredentialStore(credentials(hostKey))
        val actor = ConnectionActor(factory, store, backgroundScope)
        runCurrent()

        val actual = factory.latest.connection
        assertEquals("host", actual?.host)
        assertEquals("alice", actual?.username)
        assertContentEquals(hostKey, actual?.hostKey?.encoded)
        factory.latest.connected()
        runCurrent()

        assertIs<Link.Connecting>(actor.link.value)
        assertEquals(1, factory.latest.writes.size)
        assertEquals("tree", factory.latest.writeText().substringAfter("\"op\":\"").substringBefore("\""))

        factory.latest.receive(treeReply())
        runCurrent()

        assertEquals(Link.Up("alice@host"), actor.link.value)
    }

    @Test
    fun newCredentialsAreNotSavedAndConnectDoesNotSucceedUntilHostProtocolReplies() = runTest {
        val factory = FakeSocketFactory()
        val store = MemoryCredentialStore()
        val actor = ConnectionActor(factory, store, backgroundScope)
        val request = connection(hostKey = SshHostKey("ssh-ed25519", byteArrayOf(1), "SHA256:new"))
        val result = async { actor.connect(request) }
        runCurrent()

        factory.latest.connected()
        runCurrent()

        assertFalse(result.isCompleted)
        assertNull(store.load())
        assertIs<Link.Connecting>(actor.link.value)
        assertEquals(1, factory.latest.writes.size)
        assertEquals("tree", factory.latest.writeText().substringAfter("\"op\":\"").substringBefore("\""))

        factory.latest.receive(treeReply())
        runCurrent()

        assertEquals(ConnectResult.Success("alice@host"), result.await())
        val saved = store.load()?.connection
        assertEquals(request.copy(hostKey = null), saved?.copy(hostKey = null))
        assertEquals(request.hostKey?.algorithm, saved?.hostKey?.algorithm)
        assertEquals(request.hostKey?.fingerprint, saved?.hostKey?.fingerprint)
        assertContentEquals(request.hostKey?.encoded, saved?.hostKey?.encoded)
        assertEquals(Link.Up("alice@host"), actor.link.value)
    }

    @Test
    fun newConnectionClosedBeforeHostProtocolReplyFailsWithoutSavingOrRetrying() = runTest {
        val factory = FakeSocketFactory()
        val store = MemoryCredentialStore()
        val actor = ConnectionActor(factory, store, backgroundScope)
        val result = async {
            actor.connect(connection(hostKey = SshHostKey("ssh-ed25519", byteArrayOf(1), "SHA256:new")))
        }
        runCurrent()

        factory.latest.connected()
        runCurrent()
        factory.latest.fail(message = "xd-host missing")
        runCurrent()

        val failure = assertIs<ConnectResult.Failure>(result.await())
        assertEquals("xd-host missing", failure.message)
        assertNull(store.load())
        assertEquals(Link.Idle, actor.link.value)
        advanceTimeBy(300_000)
        runCurrent()
        assertEquals(1, factory.sockets.size)
    }

    @Test
    fun unknownSshHostKeyRequiresExplicitConfirmationWithoutSavingCredentials() = runTest {
        val factory = FakeSocketFactory()
        val store = MemoryCredentialStore()
        val actor = ConnectionActor(factory, store, backgroundScope)
        val request = SshConnection(
            host = "host",
            port = 22,
            username = "alice",
            authentication = SshAuthentication.Password("secret"),
        )
        val result = async {
            actor.connect(request)
        }
        runCurrent()

        assertEquals(request, factory.latest.connection)
        val hostKey = SshHostKey(
            algorithm = "ssh-ed25519",
            encoded = byteArrayOf(4, 5, 6),
            fingerprint = "SHA256:example",
        )
        factory.latest.fail(
            kind = SocketFailureKind.HOST_KEY_UNKNOWN,
            message = "Verify host key",
            hostKey = hostKey,
        )
        runCurrent()

        assertEquals(ConnectResult.HostKeyVerificationRequired(hostKey), result.await())
        assertNull(store.load())
        assertEquals(Link.Idle, actor.link.value)
        assertTrue(factory.latest.writes.isEmpty())
    }

    @Test
    fun backgroundingCompletesAndClearsConnectionAttempt() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(), backgroundScope)
        val result = async {
            actor.connect(connection())
        }
        runCurrent()

        actor.goBackground()
        runCurrent()

        val failure = assertIs<ConnectResult.Failure>(result.await())
        assertTrue(failure.message.contains("background"))
        assertEquals(Link.Idle, actor.link.value)
        assertTrue(factory.latest.closed)

        actor.poke()
        runCurrent()
        assertEquals(1, factory.sockets.size)
    }

    @Test
    fun eventBetweenCallAndReplyDoesNotConsumeReply() = runTest {
        val factory = FakeSocketFactory()
        val actor = connectedActor(factory)
        val event = async { actor.events.first() }
        val call = async { actor.call(com.restartfu.xd.protocol.Ops.ping()) }
        runCurrent()

        factory.latest.receive(
            """{"event":"changed"}""",
            """{"ok":true,"answer":"pong"}""",
        )
        runCurrent()

        assertEquals("changed", event.await().value.getValue("event").jsonPrimitive.content)
        assertEquals("pong", call.await().getValue("answer").jsonPrimitive.content)
    }

    @Test
    fun transientFailureUsesMobileBackoffThenReconnects() = runTest {
        val factory = FakeSocketFactory()
        val actor = connectedActor(factory)

        factory.latest.fail(message = "offline")
        runCurrent()
        assertEquals(Link.Down("offline", 2_000), actor.link.value)

        advanceTimeBy(1_999)
        runCurrent()
        assertEquals(1, factory.sockets.size)
        advanceTimeBy(1)
        runCurrent()
        assertEquals(2, factory.sockets.size)
    }

    @Test
    fun hostKeyMismatchIsFatalAndNeverRetries() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(credentials()),
            backgroundScope,
        )
        runCurrent()

        factory.latest.fail(SocketFailureKind.HOST_KEY_MISMATCH, "host key changed")
        runCurrent()
        val fatal = assertIs<Link.Fatal>(actor.link.value)

        assertEquals(FatalReason.HOST_KEY_MISMATCH, fatal.reason)
        advanceTimeBy(300_000)
        runCurrent()
        assertEquals(1, factory.sockets.size)
    }

    @Test
    fun backgroundingPreservesFatalState() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(credentials()),
            backgroundScope,
        )
        runCurrent()

        factory.latest.fail(SocketFailureKind.HOST_KEY_MISMATCH, "host key changed")
        runCurrent()
        actor.goBackground()
        runCurrent()
        actor.poke()
        runCurrent()

        assertEquals(
            FatalReason.HOST_KEY_MISMATCH,
            assertIs<Link.Fatal>(actor.link.value).reason,
        )
        assertEquals(1, factory.sockets.size)
    }

    @Test
    fun unansweredCallOnASilentStreamClosesConnectionAndFailsQueue() = runTest {
        val factory = FakeSocketFactory()
        val actor = connectedActor(factory)
        val call = async { runCatching { actor.call(com.restartfu.xd.protocol.Ops.ping()) } }
        runCurrent()

        advanceTimeBy(30_000)
        runCurrent()

        // Nothing at all arrived while it waited, so the stream is dead.
        assertTrue(call.await().isFailure)
        assertTrue(factory.latest.closed)
        assertIs<Link.Down>(actor.link.value)
    }

    @Test
    fun oneSlowCallDoesNotKillAStreamThatIsStillDelivering() = runTest {
        val factory = FakeSocketFactory()
        val actor = connectedActor(factory)
        val slow = async { runCatching { actor.call(com.restartfu.xd.protocol.Ops.ping()) } }
        runCurrent()

        // The host is demonstrably alive: it is still pushing turn output.
        advanceTimeBy(20_000)
        factory.latest.receive("""{"event":"text","chat":"chat-1","text":"hi"}""")
        runCurrent()
        advanceTimeBy(20_000)
        runCurrent()

        assertIs<CallTimedOutException>(slow.await().exceptionOrNull())
        assertFalse(factory.latest.closed)
        assertIs<Link.Up>(actor.link.value)

        // The abandoned slot must not swallow a later caller's reply. Ids run
        // readiness=1, slow=2, next=3.
        val next = async { actor.call(com.restartfu.xd.protocol.Ops.ping()) }
        runCurrent()
        factory.latest.receive("""{"ok":true,"answer":"pong","_xd_request":3}""")
        runCurrent()
        assertEquals("pong", next.await().getValue("answer").jsonPrimitive.content)
    }

    @Test
    fun callerCancellationDoesNotCloseSharedConnection() = runTest {
        val factory = FakeSocketFactory()
        val actor = connectedActor(factory)
        val abandoned = async {
            actor.call(com.restartfu.xd.protocol.Ops.ping())
        }
        val survivor = async {
            actor.call(com.restartfu.xd.protocol.Ops.ping())
        }
        runCurrent()

        abandoned.cancel()
        runCurrent()
        assertFalse(factory.latest.closed)
        assertIs<Link.Up>(actor.link.value)

        factory.latest.receive(
            """{"ok":true,"answer":"ignored"}""",
            """{"ok":true,"answer":"pong"}""",
        )
        runCurrent()

        assertEquals(
            "pong",
            survivor.await().getValue("answer").jsonPrimitive.content,
        )
        assertFalse(factory.latest.closed)
    }

    @Test
    fun cancelledCallerStillLeavesAnActiveConnectionWatchdog() = runTest {
        val factory = FakeSocketFactory()
        val actor = connectedActor(factory)
        val abandoned = async {
            actor.call(com.restartfu.xd.protocol.Ops.ping())
        }
        runCurrent()

        abandoned.cancel()
        advanceTimeBy(30_000)
        runCurrent()

        assertTrue(factory.latest.closed)
        assertIs<Link.Down>(actor.link.value)
    }

    @Test
    fun aStalledEventCollectorCannotBlockTheConnectionActor() = runTest {
        val factory = FakeSocketFactory()
        val actor = connectedActor(factory)
        val stalled = CompletableDeferred<Unit>()
        backgroundScope.launch {
            actor.events.collect { stalled.await() }
        }
        runCurrent()

        repeat(1_100) {
            factory.latest.receive("""{"event":"changed"}""")
            runCurrent()
        }

        assertTrue(factory.latest.closed)
        assertEquals("Inbound event buffer overflow", assertIs<Link.Down>(actor.link.value).message)
    }

    @Test
    fun malformedJsonIsProtocolFatal() = runTest {
        val factory = FakeSocketFactory()
        val actor = connectedActor(factory)

        factory.latest.receive("not-json")
        runCurrent()

        assertEquals(FatalReason.PROTOCOL, assertIs<Link.Fatal>(actor.link.value).reason)
        assertTrue(factory.latest.closed)
    }

    private suspend fun kotlinx.coroutines.test.TestScope.connectedActor(
        factory: FakeSocketFactory,
    ): ConnectionActor {
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(credentials()),
            backgroundScope,
        )
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receive(treeReply())
        runCurrent()
        assertIs<Link.Up>(actor.link.value)
        return actor
    }

    private fun credentials(
        hostKey: ByteArray = byteArrayOf(1, 2, 3),
    ) = StoredCredentials(
        connection(hostKey = SshHostKey("ssh-ed25519", hostKey, "SHA256:test")),
    )

    private fun connection(hostKey: SshHostKey? = null) = SshConnection(
        host = "host",
        port = 22,
        username = "alice",
        authentication = SshAuthentication.Password("secret"),
        hostKey = hostKey,
    )

    private fun treeReply() = """{"ok":true,"nodes":[],"_xd_request":1}"""
}
