package com.restartfu.xd.net

import com.jcraft.jsch.ChannelExec
import com.jcraft.jsch.HostKey
import com.jcraft.jsch.HostKeyRepository
import com.jcraft.jsch.JSch
import com.jcraft.jsch.JSchChangedHostKeyException
import com.jcraft.jsch.JSchException
import com.jcraft.jsch.JSchUnknownHostKeyException
import com.jcraft.jsch.Session
import com.jcraft.jsch.UserInfo
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

public class AndroidSshSocketFactory(
    private val connectTimeoutMillis: Int = 10_000,
) : PlatformSocketFactory {
    init {
        require(connectTimeoutMillis > 0) { "Connect timeout must be positive" }
    }

    override fun create(): PlatformSocket = AndroidSshSocket(connectTimeoutMillis)
}

internal class AndroidSshSocket(
    private val connectTimeoutMillis: Int,
) : PlatformSocket {
    private val closed = AtomicBoolean(false)
    private val callbackFinished = AtomicBoolean(false)
    private val writes = LinkedBlockingQueue<ByteArray>()

    @Volatile
    private var listener: PlatformSocketListener? = null

    @Volatile
    private var session: Session? = null

    @Volatile
    private var channel: ChannelExec? = null

    @Volatile
    private var writer: Thread? = null

    override fun connect(connection: SshConnection, listener: PlatformSocketListener) {
        check(this.listener == null) { "A PlatformSocket can connect only once" }
        require(connection.host.isNotBlank()) { "Host must not be blank" }
        require(connection.port in 1..65535) { "Port must be between 1 and 65535" }
        require(connection.username.isNotBlank()) { "Username must not be blank" }
        this.listener = listener
        thread(name = "xd-mobile-ssh", isDaemon = true) {
            runConnection(connection)
        }
    }

    override fun send(bytes: ByteArray) {
        check(!closed.get()) { "SSH connection is closed" }
        check(channel?.isConnected == true) { "SSH channel is not connected" }
        writes.add(bytes.copyOf())
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        writer?.interrupt()
        channel?.disconnect()
        session?.disconnect()
    }

    private fun runConnection(connection: SshConnection) {
        val jsch = JSch()
        val repository = PinningHostKeyRepository(connection.hostKey, jsch)
        var terminalReason: SocketFailure? = null
        try {
            jsch.hostKeyRepository = repository
            configureIdentity(jsch, connection.authentication)
            val activeSession = jsch.getSession(connection.username, connection.host, connection.port)
            session = activeSession
            activeSession.setConfig("StrictHostKeyChecking", "yes")
            activeSession.setConfig(
                "PreferredAuthentications",
                when (connection.authentication) {
                    is SshAuthentication.Password -> "password"
                    is SshAuthentication.PrivateKey -> "publickey"
                },
            )
            if (connection.authentication is SshAuthentication.Password) {
                activeSession.setPassword(connection.authentication.value.encodeToByteArray())
            }
            activeSession.connect(connectTimeoutMillis)
            if (closed.get()) throw EOFException("SSH connection was closed")

            val activeChannel = activeSession.openChannel("exec") as ChannelExec
            channel = activeChannel
            activeChannel.setCommand(XD_HOST_COMMAND)
            val stdout = activeChannel.inputStream
            val stderr = activeChannel.extInputStream
            val stdin = activeChannel.outputStream
            val stderrBuffer = ByteArrayOutputStream()
            val stderrThread = drainStderr(stderr, stderrBuffer)
            activeChannel.connect(connectTimeoutMillis)
            if (closed.get()) throw EOFException("SSH connection was closed")

            writer = thread(name = "xd-mobile-ssh-writer", isDaemon = true) {
                runWriter(stdin)
            }
            listener?.onConnected()
            readStdout(stdout)
            stderrThread.join(500)
            if (!closed.get()) {
                val exit = activeChannel.exitStatus
                terminalReason = if (exit > 0) {
                    val detail = stderrBuffer.toString(Charsets.UTF_8.name()).trim()
                    SocketFailure(
                        SocketFailureKind.IO,
                        if (detail.isEmpty()) {
                            "The remote xd host exited with status $exit"
                        } else {
                            "The remote xd host exited with status $exit: $detail"
                        },
                    )
                } else {
                    SocketFailure(SocketFailureKind.IO, "The remote xd host closed the SSH channel")
                }
            }
        } catch (error: Throwable) {
            if (!closed.get()) terminalReason = error.toSocketFailure(repository.presented)
        } finally {
            close()
            finish(terminalReason)
        }
    }

    private fun configureIdentity(jsch: JSch, authentication: SshAuthentication) {
        if (authentication !is SshAuthentication.PrivateKey) return
        val privateKey = authentication.bytes.copyOf()
        val passphrase = authentication.passphrase?.encodeToByteArray()
        try {
            jsch.addIdentity("xd-mobile", privateKey, null, passphrase)
        } finally {
            privateKey.fill(0)
            passphrase?.fill(0)
        }
    }

    private fun runWriter(output: OutputStream) {
        try {
            while (!closed.get()) {
                val bytes = writes.take()
                output.write(bytes)
                output.flush()
            }
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
        } catch (error: Throwable) {
            if (!closed.get()) {
                close()
                finish(error.toSocketFailure(null))
            }
        }
    }

    private fun readStdout(input: InputStream) {
        val buffer = ByteArray(32 * 1024)
        while (!closed.get()) {
            val count = input.read(buffer)
            if (count < 0) return
            if (count > 0) listener?.onBytes(buffer.copyOf(count))
        }
    }

    private fun drainStderr(input: InputStream, destination: ByteArrayOutputStream): Thread =
        thread(name = "xd-mobile-ssh-stderr", isDaemon = true) {
            val buffer = ByteArray(4096)
            try {
                while (!closed.get()) {
                    val count = input.read(buffer)
                    if (count < 0) return@thread
                    if (destination.size() < MAX_STDERR_BYTES) {
                        destination.write(buffer, 0, minOf(count, MAX_STDERR_BYTES - destination.size()))
                    }
                }
            } catch (_: IOException) {
                // Closing the channel interrupts the stderr reader.
            }
        }

    private fun finish(reason: SocketFailure?) {
        if (callbackFinished.compareAndSet(false, true)) listener?.onClosed(reason)
    }
}

internal class PinningHostKeyRepository(
    private val expected: SshHostKey?,
    private val jsch: JSch = JSch(),
) : HostKeyRepository {
    @Volatile
    var presented: SshHostKey? = null
        private set

    override fun check(host: String, key: ByteArray): Int {
        val hostKey = HostKey(host, key)
        presented = SshHostKey(
            algorithm = hostKey.type,
            encoded = key.copyOf(),
            fingerprint = hostKey.getFingerPrint(jsch),
        )
        val pin = expected ?: return HostKeyRepository.NOT_INCLUDED
        return if (pin.algorithm == hostKey.type && pin.encoded.contentEquals(key)) {
            HostKeyRepository.OK
        } else {
            HostKeyRepository.CHANGED
        }
    }

    override fun add(hostkey: HostKey, ui: UserInfo?) = Unit
    override fun remove(host: String?, type: String?) = Unit
    override fun remove(host: String?, type: String?, key: ByteArray?) = Unit
    override fun getKnownHostsRepositoryID(): String = "xd-mobile-pinned-host-key"
    override fun getHostKey(): Array<HostKey> = emptyArray()
    override fun getHostKey(host: String?, type: String?): Array<HostKey> = emptyArray()
}

private fun Throwable.toSocketFailure(presented: SshHostKey?): SocketFailure {
    val message = message ?: "SSH connection failed"
    return when {
        this is JSchUnknownHostKeyException -> SocketFailure(
            SocketFailureKind.HOST_KEY_UNKNOWN,
            "Verify the SSH host key before connecting",
            presented,
        )
        this is JSchChangedHostKeyException -> SocketFailure(
            SocketFailureKind.HOST_KEY_MISMATCH,
            "The SSH host key does not match the pinned key",
            presented,
        )
        this is JSchException && (
            message.contains("Auth fail", ignoreCase = true) ||
                message.contains("Auth cancel", ignoreCase = true) ||
                message.contains("authentication", ignoreCase = true)
        ) ->
            SocketFailure(SocketFailureKind.AUTHENTICATION, "SSH authentication failed")
        this is UnknownHostException || this is NoRouteToHostException ||
            this is ConnectException || this is SocketTimeoutException ||
            cause is UnknownHostException || cause is NoRouteToHostException ||
            cause is ConnectException || cause is SocketTimeoutException ->
            SocketFailure(SocketFailureKind.UNREACHABLE, message)
        this is IOException || this is JSchException -> SocketFailure(SocketFailureKind.IO, message)
        else -> SocketFailure(SocketFailureKind.IO, message)
    }
}

internal const val XD_HOST_COMMAND: String =
    "exec \"\$HOME/.local/share/xd/runtime/v1/xd-host\" stdio " +
        "--data \"\$HOME/.local/share/xd\""

private const val MAX_STDERR_BYTES = 16 * 1024
