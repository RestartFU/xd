package com.restartfu.xd.net

import com.restartfu.xd.credentials.MemoryCredentialStore
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
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.jsonPrimitive

@OptIn(ExperimentalCoroutinesApi::class)
class ConnectionActorTest {
    @Test
    fun authenticatesStoredCredentialsWithPinnedCertificate() = runTest {
        val certificate = byteArrayOf(9, 8, 7)
        val factory = FakeSocketFactory()
        val store = MemoryCredentialStore(credentials(certificate))
        val actor = ConnectionActor(factory, store, backgroundScope)
        runCurrent()

        assertContentEquals(certificate, factory.latest.pin)
        factory.latest.connected(certificate)
        runCurrent()
        assertEquals(
            """{"op":"hello","token":"token","_xd_request":1}""" + "\n",
            factory.latest.writes.single().decodeToString(),
        )

        factory.latest.receive("""{"ok":true,"device":"Pixel","version":1}""")
        runCurrent()
        runCurrent()

        assertEquals(Link.Up("Pixel"), actor.link.value)
    }

    @Test
    fun pairingUsesOneUnpinnedGreetingAndPersistsCertificate() = runTest {
        val factory = FakeSocketFactory()
        val store = MemoryCredentialStore()
        val actor = ConnectionActor(
            factory,
            store,
            backgroundScope,
            deviceName = "Test device",
        )
        val result = async {
            actor.pair("daemon", 4001, "ABCD-EFGH")
        }
        runCurrent()

        assertNull(factory.latest.pin)
        val certificate = byteArrayOf(4, 5, 6)
        factory.latest.connected(certificate)
        runCurrent()
        assertEquals(
            """{"op":"pair","code":"ABCD-EFGH","name":"Test device","_xd_request":1}""" + "\n",
            factory.latest.writes.single().decodeToString(),
        )

        factory.latest.receive("""{"ok":true,"token":"new-token","device":"Workstation"}""")
        runCurrent()
        runCurrent()

        assertEquals(PairResult.Success("Workstation"), result.await())
        assertEquals(Link.Up("Workstation"), actor.link.value)
        assertEquals("new-token", store.load()?.token)
        assertContentEquals(certificate, store.load()?.certificateDer)
        assertEquals(1, factory.latest.writes.size)
    }

    @Test
    fun invalidPairingNameCompletesWithFailure() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(),
            backgroundScope,
            deviceName = " ",
        )
        val result = async {
            actor.pair("daemon", 4001, "ABCD-EFGH")
        }
        runCurrent()

        factory.latest.connected()
        runCurrent()

        val failure = assertIs<PairResult.Failure>(result.await())
        assertEquals("Device name must not be blank", failure.message)
        assertEquals(Link.Idle, actor.link.value)
        assertTrue(factory.latest.closed)
    }

    @Test
    fun backgroundingCompletesAndClearsPairingAttempt() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(), backgroundScope)
        val result = async {
            actor.pair("daemon", 4001, "ABCD-EFGH")
        }
        runCurrent()

        actor.goBackground()
        runCurrent()

        val failure = assertIs<PairResult.Failure>(result.await())
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
    fun pinMismatchIsFatalAndNeverRetries() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(
            factory,
            MemoryCredentialStore(credentials()),
            backgroundScope,
        )
        runCurrent()

        factory.latest.fail(SocketFailureKind.PIN_MISMATCH, "certificate changed")
        runCurrent()
        val fatal = assertIs<Link.Fatal>(actor.link.value)

        assertEquals(FatalReason.PIN_MISMATCH, fatal.reason)
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

        factory.latest.fail(SocketFailureKind.PIN_MISMATCH, "certificate changed")
        runCurrent()
        actor.goBackground()
        runCurrent()
        actor.poke()
        runCurrent()

        assertEquals(
            FatalReason.PIN_MISMATCH,
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

        // The daemon is demonstrably alive: it is still pushing turn output.
        advanceTimeBy(20_000)
        factory.latest.receive("""{"event":"text","chat":"chat-1","text":"hi"}""")
        runCurrent()
        advanceTimeBy(20_000)
        runCurrent()

        assertIs<CallTimedOutException>(slow.await().exceptionOrNull())
        assertFalse(factory.latest.closed)
        assertIs<Link.Up>(actor.link.value)

        // The abandoned slot must not swallow a later caller's reply. Ids run
        // hello=1, slow=2, next=3.
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
    fun inboundBufferOverflowClosesSocket() = runTest {
        val factory = FakeSocketFactory()
        connectedActor(factory)

        repeat(1_100) {
            factory.latest.receive("""{"event":"changed"}""")
        }

        assertTrue(factory.latest.closed)
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

    @Test
    fun malformedPairingReplyFailsInsteadOfLeavingCallerWaiting() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(), backgroundScope)
        val result = async {
            actor.pair("daemon", 4001, "ABCD-EFGH")
        }
        runCurrent()
        factory.latest.connected()
        runCurrent()

        factory.latest.receive("not-json")
        runCurrent()

        assertIs<PairResult.Failure>(result.await())
        assertEquals(FatalReason.PROTOCOL, assertIs<Link.Fatal>(actor.link.value).reason)
    }

    @Test
    fun pairingGreetingTimesOut() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(), backgroundScope)
        val result = async {
            actor.pair("daemon", 4001, "ABCD-EFGH")
        }
        runCurrent()
        factory.latest.connected()
        runCurrent()

        advanceTimeBy(15_000)
        runCurrent()

        val failure = assertIs<PairResult.Failure>(result.await())
        assertTrue(failure.message.contains("Timed out", ignoreCase = true))
        assertEquals(Link.Idle, actor.link.value)
        assertTrue(factory.latest.closed)
    }

    @Test
    fun refusedPairingCanBeRetried() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(), backgroundScope)
        val first = async {
            actor.pair("daemon", 4001, "BAD1-CODE")
        }
        runCurrent()
        factory.latest.connected()
        runCurrent()
        factory.latest.receive("""{"ok":false,"error":"expired code"}""")
        runCurrent()

        assertIs<PairResult.Failure>(first.await())
        assertEquals(Link.Idle, actor.link.value)

        val second = async {
            actor.pair("daemon", 4001, "ABCD-EFGH")
        }
        runCurrent()
        assertEquals(2, factory.sockets.size)
        actor.goBackground()
        runCurrent()
        assertIs<PairResult.Failure>(second.await())
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
        factory.latest.receive("""{"ok":true,"device":"Pixel","version":1}""")
        runCurrent()
        runCurrent()
        assertIs<Link.Up>(actor.link.value)
        return actor
    }

    private fun credentials(
        certificate: ByteArray = byteArrayOf(1, 2, 3),
    ) = StoredCredentials(
        host = "daemon",
        port = 4001,
        token = "token",
        certificateDer = certificate,
    )
}
