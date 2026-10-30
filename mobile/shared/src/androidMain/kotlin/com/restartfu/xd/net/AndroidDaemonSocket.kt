package com.restartfu.xd.net

import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import java.net.InetSocketAddress
import java.net.Socket
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509TrustManager

public class AndroidDaemonSocketFactory : PlatformSocketFactory {
    override fun create(): PlatformSocket = AndroidDaemonSocket()
}

/** TLS verifies the certificate in the invitation before any pairing/token frame. */
internal class AndroidDaemonSocket : PlatformSocket {
    private val lock = Any()
    private val writes = ThreadPoolExecutor(1, 1, 0L, TimeUnit.MILLISECONDS, ArrayBlockingQueue(256))
    @Volatile private var closed = false
    @Volatile private var socket: Socket? = null
    private var listener: PlatformSocketListener? = null

    override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
        this.listener = listener
        Thread({
            try {
                val authentication = connection.authentication as? SshAuthentication.Paired
                    ?: throw CertificateException("This saved SSH connection needs a new daemon pairing code")
                val trust = object : X509TrustManager {
                    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
                    override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {
                        throw CertificateException("Client certificates are not used")
                    }
                    override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
                        val leaf = chain.firstOrNull() ?: throw CertificateException("Host did not present a certificate")
                        val fingerprint = MessageDigest.getInstance("SHA-256").digest(leaf.encoded)
                            .joinToString("") { "%02x".format(it.toInt() and 255) }
                        if (!fingerprint.equals(authentication.fingerprint, ignoreCase = true)) {
                            throw CertificateException("Host certificate changed. Pair with the host again.")
                        }
                    }
                }
                val context = SSLContext.getInstance("TLS").apply { init(null, arrayOf(trust), null) }
                val raw = Socket()
                synchronized(lock) { if (closed) { raw.close(); return@Thread }; socket = raw }
                raw.connect(InetSocketAddress(connection.host, connection.port), 10_000)
                val tls = context.socketFactory.createSocket(raw, connection.host, connection.port, true) as SSLSocket
                synchronized(lock) { if (closed) { tls.close(); return@Thread }; socket = tls }
                tls.soTimeout = 10_000
                tls.startHandshake()
                tls.soTimeout = 0
                if (closed) return@Thread
                listener.onConnected()
                val buffer = ByteArray(64 * 1024)
                while (!closed) {
                    val count = tls.inputStream.read(buffer)
                    if (count < 0) break
                    if (count > 0) listener.onBytes(buffer.copyOf(count))
                }
                if (!closed) listener.onClosed(SocketFailure(SocketFailureKind.IO, "Host disconnected"))
            } catch (error: Throwable) {
                if (!closed) {
                    val certificateFailure = generateSequence(error) { it.cause }.any { it is CertificateException }
                    listener.onClosed(SocketFailure(
                        if (certificateFailure) SocketFailureKind.HOST_KEY_MISMATCH else SocketFailureKind.IO,
                        error.message ?: "Could not connect to the xd daemon",
                    ))
                }
            } finally {
                close()
            }
        }, "xd-daemon-reader").start()
    }

    override fun send(bytes: ByteArray) {
        check(!closed) { "Daemon connection closed" }
        writes.execute {
            try {
                val output = checkNotNull(socket).getOutputStream()
                output.write(bytes)
                output.flush()
            } catch (error: Throwable) {
                if (!closed) listener?.onClosed(SocketFailure(SocketFailureKind.IO, error.message ?: "Write failed"))
                close()
            }
        }
    }

    override fun close() {
        synchronized(lock) {
            if (closed) return
            closed = true
            runCatching { socket?.close() }
            socket = null
            writes.shutdownNow()
        }
    }
}
