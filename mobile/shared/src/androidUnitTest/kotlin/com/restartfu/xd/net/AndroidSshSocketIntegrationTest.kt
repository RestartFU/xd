package com.restartfu.xd.net

import com.jcraft.jsch.JSch
import com.jcraft.jsch.KeyPair
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import java.io.ByteArrayOutputStream
import java.security.PublicKey
import java.security.KeyPairGenerator
import java.util.Base64
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import org.apache.sshd.common.config.keys.KeyUtils
import org.apache.sshd.common.config.keys.PublicKeyEntry
import org.apache.sshd.common.keyprovider.KeyPairProvider
import org.apache.sshd.server.SshServer
import org.apache.sshd.server.auth.password.PasswordAuthenticator
import org.apache.sshd.server.auth.pubkey.PublickeyAuthenticator
import org.apache.sshd.server.channel.ChannelSession
import org.apache.sshd.server.command.Command
import org.apache.sshd.server.command.CommandFactory
import org.apache.sshd.server.Environment

class AndroidSshSocketIntegrationTest {
    private var server: SshServer? = null

    @AfterTest
    fun stopServer() {
        server?.stop(true)
        server = null
    }

    @Test
    fun unknownHostKeyIsRejectedBeforeAuthentication() {
        val authAttempts = AtomicInteger(0)
        val fixture = startServer(
            passwordAuthenticator = PasswordAuthenticator { _, _, _ ->
                authAttempts.incrementAndGet()
                true
            },
        )
        val listener = RecordingListener()

        AndroidSshSocket(2_000).connect(
            fixture.connection(SshAuthentication.Password("secret"), hostKey = null),
            listener,
        )

        val failure = listener.awaitClosed()
        assertEquals(SocketFailureKind.HOST_KEY_UNKNOWN, failure?.kind)
        assertNotNull(failure?.hostKey)
        assertEquals(0, authAttempts.get())
        assertTrue(listener.bytes().isEmpty())
    }

    @Test
    fun passwordAuthExecStdioStreamsUntilClosed() {
        val fixture = startServer(
            passwordAuthenticator = PasswordAuthenticator { username, password, _ ->
                username == "danick" && password == "secret"
            },
        )
        val listener = RecordingListener()
        val socket = AndroidSshSocket(2_000)

        socket.connect(fixture.connection(SshAuthentication.Password("secret")), listener)
        listener.awaitConnectedOrFail()
        assertEquals(xdHostCommand("xd"), fixture.command.awaitCommand())

        socket.send("ping".encodeToByteArray())
        assertTrue(listener.awaitBytesContaining("remote-ready\nping".encodeToByteArray()))

        socket.close()
        assertNull(listener.awaitClosed())
    }

    @Test
    fun encryptedPrivateKeyWithPassphraseAuthenticatesAndExecs() {
        val key = generateEncryptedClientKey("key-passphrase")
        val clientPublicKeyLine = "ssh-rsa ${Base64.getEncoder().encodeToString(key.publicKeyBlob)}"
        val fixture = startServer(
            publickeyAuthenticator = PublickeyAuthenticator { username, publicKey, _ ->
                username == "danick" && PublicKeyEntry.toString(publicKey) == clientPublicKeyLine
            },
        )
        val listener = RecordingListener()
        val socket = AndroidSshSocket(2_000)

        socket.connect(
            fixture.connection(SshAuthentication.PrivateKey(key.privateKeyPem, "key-passphrase")),
            listener,
        )
        listener.awaitConnectedOrFail()
        assertEquals(xdHostCommand("xd"), fixture.command.awaitCommand())
        assertTrue(listener.awaitBytesContaining("remote-ready\n".encodeToByteArray()))

        socket.close()
        assertNull(listener.awaitClosed())
    }

    private fun startServer(
        passwordAuthenticator: PasswordAuthenticator = PasswordAuthenticator { _, _, _ -> false },
        publickeyAuthenticator: PublickeyAuthenticator = PublickeyAuthenticator { _, _, _ -> false },
    ): ServerFixture {
        val command = RecordingCommandFactory()
        val hostKeyPair = KeyPairGenerator.getInstance(KeyUtils.RSA_ALGORITHM).apply { initialize(2048) }.generateKeyPair()
        val sshd = SshServer.setUpDefaultServer().apply {
            host = "127.0.0.1"
            port = 0
            keyPairProvider = KeyPairProvider { Collections.singletonList(hostKeyPair) }
            this.passwordAuthenticator = passwordAuthenticator
            this.publickeyAuthenticator = publickeyAuthenticator
            commandFactory = command
            start()
        }
        server = sshd
        return ServerFixture(sshd.port, toPinnedHostKey(hostKeyPair.public), command)
    }

    private fun toPinnedHostKey(publicKey: PublicKey): SshHostKey {
        val encoded = Base64.getDecoder().decode(PublicKeyEntry.toString(publicKey).substringAfter(' ').substringBefore(' '))
        val jschHostKey = com.jcraft.jsch.HostKey("127.0.0.1", encoded)
        return SshHostKey(jschHostKey.type, encoded, jschHostKey.getFingerPrint(JSch()))
    }

    private fun generateEncryptedClientKey(passphrase: String): EncryptedClientKey {
        val output = ByteArrayOutputStream()
        val pair = KeyPair.genKeyPair(JSch(), KeyPair.RSA, 2048)
        pair.writePrivateKey(output, passphrase.encodeToByteArray())
        return EncryptedClientKey(output.toByteArray(), pair.publicKeyBlob)
    }

    private data class EncryptedClientKey(val privateKeyPem: ByteArray, val publicKeyBlob: ByteArray)

    private data class ServerFixture(
        val port: Int,
        val hostKey: SshHostKey,
        val command: RecordingCommandFactory,
    ) {
        fun connection(authentication: SshAuthentication, hostKey: SshHostKey? = this.hostKey) = SshConnection(
            host = "127.0.0.1",
            port = port,
            username = "danick",
            authentication = authentication,
            hostKey = hostKey,
        )
    }

    private class RecordingListener : PlatformSocketListener {
        private val connected = CountDownLatch(1)
        private val closed = CountDownLatch(1)
        private val output = ByteArrayOutputStream()
        @Volatile private var failure: SocketFailure? = null

        override fun onConnected() { connected.countDown() }
        override fun onBytes(bytes: ByteArray) { synchronized(output) { output.write(bytes) } }
        override fun onClosed(reason: SocketFailure?) { failure = reason; closed.countDown() }

        fun awaitConnectedOrFail() {
            if (connected.await(5, TimeUnit.SECONDS)) return
            if (closed.count == 0L) error("SSH socket closed before connect: $failure")
            error("SSH socket did not connect or close")
        }
        fun awaitClosed(): SocketFailure? { assertTrue(closed.await(5, TimeUnit.SECONDS)); return failure }
        fun bytes(): ByteArray = synchronized(output) { output.toByteArray() }
        fun awaitBytesContaining(expected: ByteArray): Boolean {
            val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5)
            while (System.nanoTime() < deadline) {
                if (bytes().indexOf(expected) >= 0) return true
                Thread.sleep(20)
            }
            return bytes().indexOf(expected) >= 0
        }
    }

    private class RecordingCommandFactory : CommandFactory {
        private val commandLatch = CountDownLatch(1)
        @Volatile private var command: String? = null

        override fun createCommand(channel: ChannelSession, command: String): Command {
            this.command = command
            commandLatch.countDown()
            return EchoCommand()
        }

        fun awaitCommand(): String? {
            assertTrue(commandLatch.await(5, TimeUnit.SECONDS))
            return command
        }
    }

    private class EchoCommand : Command {
        private lateinit var input: java.io.InputStream
        private lateinit var output: java.io.OutputStream
        private var thread: Thread? = null

        override fun setInputStream(`in`: java.io.InputStream) { input = `in` }
        override fun setOutputStream(out: java.io.OutputStream) { output = out }
        override fun setErrorStream(err: java.io.OutputStream) = Unit
        override fun setExitCallback(callback: org.apache.sshd.server.ExitCallback) = Unit

        override fun start(channel: ChannelSession, env: Environment) {
            thread = thread(isDaemon = true) {
                output.write("remote-ready\n".encodeToByteArray())
                output.flush()
                val buffer = ByteArray(1024)
                while (!Thread.currentThread().isInterrupted) {
                    val count = input.read(buffer)
                    if (count < 0) return@thread
                    output.write(buffer, 0, count)
                    output.flush()
                }
            }
        }

        override fun destroy(channel: ChannelSession) {
            thread?.interrupt()
        }
    }
}

private fun ByteArray.indexOf(needle: ByteArray): Int {
    if (needle.isEmpty()) return 0
    if (needle.size > size) return -1
    for (start in 0..size - needle.size) {
        var matched = true
        for (offset in needle.indices) {
            if (this[start + offset] != needle[offset]) {
                matched = false
                break
            }
        }
        if (matched) return start
    }
    return -1
}
