package com.restartfu.xd.net

import java.io.ByteArrayOutputStream
import java.net.ServerSocket
import java.security.KeyStore
import java.security.cert.X509Certificate
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLServerSocket
import kotlin.concurrent.thread
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class AndroidSocketTest {
    @Test
    fun unpinnedPairingReportsLeafAndReadsRawBytes() {
        val fixture = fixture()
        val server = fixture.listen { socket ->
            socket.startHandshake()
            socket.outputStream.write("one\n".encodeToByteArray())
            socket.outputStream.flush()
        }
        val connected = AtomicReference<ByteArray>()
        val received = ByteArrayOutputStream()
        val done = CountDownLatch(1)

        try {
            AndroidSocket(2_000).connect(
                "127.0.0.1",
                server.localPort,
                null,
                listener(
                    connected = { connected.set(it) },
                    bytes = { chunk ->
                        synchronized(received) {
                            received.write(chunk)
                        }
                    },
                    closed = { done.countDown() },
                ),
            )

            assertTrue(done.await(5, TimeUnit.SECONDS))
            assertContentEquals(fixture.certificateDer, connected.get())
            assertEquals(
                "one\n",
                synchronized(received) { received.toByteArray().decodeToString() },
            )
        } finally {
            server.close()
        }
    }

    @Test
    fun exactLeafPinCompletesHandshake() {
        val fixture = fixture()
        val server = fixture.listen { socket ->
            socket.startHandshake()
            socket.close()
        }
        val connected = CountDownLatch(1)
        val done = CountDownLatch(1)
        val failure = AtomicReference<SocketFailure?>()

        try {
            AndroidSocket(2_000).connect(
                "127.0.0.1",
                server.localPort,
                fixture.certificateDer,
                listener(
                    connected = { connected.countDown() },
                    closed = {
                        failure.set(it)
                        done.countDown()
                    },
                ),
            )

            assertTrue(connected.await(5, TimeUnit.SECONDS))
            assertTrue(done.await(5, TimeUnit.SECONDS))
            assertEquals(null, failure.get())
        } finally {
            server.close()
        }
    }

    @Test
    fun differentLeafPinIsReportedAsPinMismatch() {
        val fixture = fixture()
        val server = fixture.listen { socket ->
            runCatching { socket.startHandshake() }
        }
        val done = CountDownLatch(1)
        val failure = AtomicReference<SocketFailure?>()

        try {
            AndroidSocket(2_000).connect(
                "127.0.0.1",
                server.localPort,
                byteArrayOf(1, 2, 3),
                listener(
                    closed = {
                        failure.set(it)
                        done.countDown()
                    },
                ),
            )

            assertTrue(done.await(5, TimeUnit.SECONDS))
            assertEquals(SocketFailureKind.PIN_MISMATCH, assertNotNull(failure.get()).kind)
        } finally {
            server.close()
        }
    }

    @Test
    fun tlsHandshakeTimesOutWhenPeerDoesNotRespond() {
        val server = ServerSocket(0)
        val accepted = CountDownLatch(1)
        val releaseServer = CountDownLatch(1)
        val serverThread = thread(name = "xd-test-stalled-tls-server", isDaemon = true) {
            server.accept().use {
                accepted.countDown()
                releaseServer.await(5, TimeUnit.SECONDS)
            }
        }
        val done = CountDownLatch(1)
        val failure = AtomicReference<SocketFailure?>()

        try {
            AndroidSocket(
                connectTimeoutMillis = 2_000,
                handshakeTimeoutMillis = 100,
            ).connect(
                "127.0.0.1",
                server.localPort,
                null,
                listener(
                    closed = {
                        failure.set(it)
                        done.countDown()
                    },
                ),
            )

            assertTrue(accepted.await(5, TimeUnit.SECONDS))
            assertTrue(done.await(5, TimeUnit.SECONDS))
            assertEquals(SocketFailureKind.UNREACHABLE, assertNotNull(failure.get()).kind)
        } finally {
            releaseServer.countDown()
            server.close()
            serverThread.join(5_000)
        }
    }

    private fun listener(
        connected: (ByteArray) -> Unit = {},
        bytes: (ByteArray) -> Unit = {},
        closed: (SocketFailure?) -> Unit,
    ): PlatformSocketListener = object : PlatformSocketListener {
        override fun onConnected(leafCertificateDer: ByteArray) = connected(leafCertificateDer)
        override fun onBytes(chunk: ByteArray) = bytes(chunk)
        override fun onClosed(reason: SocketFailure?) = closed(reason)
    }

    private fun fixture(): TlsFixture {
        val password = "changeit".toCharArray()
        val store = KeyStore.getInstance("PKCS12")
        javaClass.classLoader
            ?.getResourceAsStream("loopback.p12")
            .use { stream ->
                store.load(assertNotNull(stream), password)
            }
        val managers = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm())
        managers.init(store, password)
        val context = SSLContext.getInstance("TLS")
        context.init(managers.keyManagers, null, null)
        val certificate = store.getCertificate("xd-test") as X509Certificate
        return TlsFixture(context, certificate.encoded)
    }

    private data class TlsFixture(
        val context: SSLContext,
        val certificateDer: ByteArray,
    ) {
        fun listen(block: (javax.net.ssl.SSLSocket) -> Unit): SSLServerSocket {
            val server = context.serverSocketFactory.createServerSocket(0) as SSLServerSocket
            thread(name = "xd-test-tls-server", isDaemon = true) {
                server.accept().use { socket ->
                    block(socket as javax.net.ssl.SSLSocket)
                }
            }
            return server
        }
    }
}
