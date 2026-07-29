package com.restartfu.xd.net

import java.io.EOFException
import java.io.IOException
import java.net.ConnectException
import java.net.InetSocketAddress
import java.net.NoRouteToHostException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.security.cert.CertificateException
import java.util.concurrent.atomic.AtomicBoolean
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLException
import javax.net.ssl.SSLSocket

public class AndroidSocketFactory(
    private val connectTimeoutMillis: Int = 10_000,
) : PlatformSocketFactory {
    override fun create(): PlatformSocket = AndroidSocket(connectTimeoutMillis)
}

internal class AndroidSocket(
    private val connectTimeoutMillis: Int,
) : PlatformSocket {
    private val closed = AtomicBoolean(false)
    private val callbackFinished = AtomicBoolean(false)
    private val writeLock = Any()

    @Volatile
    private var socket: SSLSocket? = null

    @Volatile
    private var listener: PlatformSocketListener? = null

    override fun connect(
        host: String,
        port: Int,
        pinnedCertificateDer: ByteArray?,
        listener: PlatformSocketListener,
    ) {
        check(this.listener == null) { "A PlatformSocket can connect only once" }
        require(host.isNotBlank()) { "Host must not be blank" }
        require(port in 1..65535) { "Port must be between 1 and 65535" }
        this.listener = listener

        Thread(
            {
                runConnection(host, port, pinnedCertificateDer?.copyOf())
            },
            "xd-mobile-tls",
        ).apply {
            isDaemon = true
            start()
        }
    }

    override fun send(bytes: ByteArray) {
        check(!closed.get()) { "Socket is closed" }
        synchronized(writeLock) {
            val active = socket ?: error("Socket is not connected")
            active.outputStream.write(bytes)
            active.outputStream.flush()
        }
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        try {
            socket?.close()
        } catch (_: IOException) {
            // Closing is best effort and idempotent.
        }
    }

    private fun runConnection(
        host: String,
        port: Int,
        pin: ByteArray?,
    ) {
        try {
            if (closed.get()) throw EOFException("Socket was closed")
            val context = SSLContext.getInstance("TLS")
            context.init(null, arrayOf(PinningTrustManager(pin)), null)
            val active = context.socketFactory.createSocket() as SSLSocket
            socket = active
            if (closed.get()) {
                active.close()
                throw EOFException("Socket was closed")
            }
            active.tcpNoDelay = true
            active.sslParameters = active.sslParameters.apply {
                endpointIdentificationAlgorithm = null
            }
            active.connect(InetSocketAddress(host, port), connectTimeoutMillis)
            active.startHandshake()

            if (closed.get()) throw EOFException("Socket was closed")
            val certificate = active.session.peerCertificates.firstOrNull()?.encoded
                ?: throw SSLException("The daemon supplied no certificate")
            listener?.onConnected(certificate.copyOf())

            val buffer = ByteArray(READ_BUFFER_BYTES)
            while (!closed.get()) {
                val count = active.inputStream.read(buffer)
                if (count < 0) {
                    finish(null)
                    return
                }
                if (count > 0) listener?.onBytes(buffer.copyOf(count))
            }
            finish(SocketFailure(SocketFailureKind.CANCELLED, "Socket closed"))
        } catch (error: Throwable) {
            finish(error.toSocketFailure())
        } finally {
            try {
                socket?.close()
            } catch (_: IOException) {
                // Reader already ended.
            }
            socket = null
        }
    }

    private fun finish(reason: SocketFailure?) {
        if (callbackFinished.compareAndSet(false, true)) {
            listener?.onClosed(reason)
        }
    }

    private fun Throwable.toSocketFailure(): SocketFailure {
        val kind = when {
            hasCause<PinMismatchCertificateException>() -> SocketFailureKind.PIN_MISMATCH
            closed.get() -> SocketFailureKind.CANCELLED
            this is UnknownHostException ||
                this is ConnectException ||
                this is NoRouteToHostException ||
                this is SocketTimeoutException -> SocketFailureKind.UNREACHABLE
            this is SSLException || hasCause<CertificateException>() -> SocketFailureKind.TLS
            else -> SocketFailureKind.IO
        }
        val fallback = when (kind) {
            SocketFailureKind.PIN_MISMATCH ->
                "The daemon certificate does not match the paired machine"
            SocketFailureKind.CANCELLED -> "Socket closed"
            else -> "Connection failed"
        }
        return SocketFailure(kind, message ?: fallback)
    }

    private inline fun <reified T : Throwable> Throwable.hasCause(): Boolean {
        var current: Throwable? = this
        while (current != null) {
            if (current is T) return true
            current = current.cause
        }
        return false
    }

    private companion object {
        const val READ_BUFFER_BYTES = 64 * 1024
    }
}
