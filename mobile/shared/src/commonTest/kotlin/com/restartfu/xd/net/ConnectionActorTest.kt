package com.restartfu.xd.net

import com.restartfu.xd.credentials.MemoryCredentialStore
import com.restartfu.xd.credentials.StoredCredentials
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
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
            """{"op":"hello","token":"token"}""" + "\n",
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
        val actor = ConnectionActor(factory, store, backgroundScope)
        val result = async {
            actor.pair("daemon", 4001, "ABCD-EFGH", "Phone")
        }
        runCurrent()

        assertNull(factory.latest.pin)
        val certificate = byteArrayOf(4, 5, 6)
        factory.latest.connected(certificate)
        runCurrent()
        assertEquals(
            """{"op":"pair","code":"ABCD-EFGH","name":"Phone"}""" + "\n",
            factory.latest.writes.single().decodeToString(),
        )

        factory.latest.receive("""{"ok":true,"token":"new-token"}""")
        runCurrent()
        runCurrent()

        assertEquals(PairResult.Success("Phone"), result.await())
        assertEquals(Link.Up("Phone"), actor.link.value)
        assertEquals("new-token", store.load()?.token)
        assertContentEquals(certificate, store.load()?.certificateDer)
        assertEquals(1, factory.latest.writes.size)
    }

    @Test
    fun backgroundingCompletesAndClearsPairingAttempt() = runTest {
        val factory = FakeSocketFactory()
        val actor = ConnectionActor(factory, MemoryCredentialStore(), backgroundScope)
        val result = async {
            actor.pair("daemon", 4001, "ABCD-EFGH", "Phone")
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
            actor.pair("daemon", 4001, "ABCD-EFGH", "Phone")
        }
        runCurrent()
        factory.latest.connected()
        runCurrent()

        factory.latest.receive("not-json")
        runCurrent()

        assertIs<PairResult.Failure>(result.await())
        assertEquals(FatalReason.PROTOCOL, assertIs<Link.Fatal>(actor.link.value).reason)
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
