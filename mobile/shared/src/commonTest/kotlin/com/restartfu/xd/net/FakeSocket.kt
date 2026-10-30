package com.restartfu.xd.net

import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey

internal class FakeSocketFactory : PlatformSocketFactory {
    val sockets = mutableListOf<FakeSocket>()

    override fun create(): PlatformSocket = FakeSocket().also(sockets::add)

    val latest: FakeSocket
        get() = sockets.last()
}

internal class FakeSocket : PlatformSocket {
    var connection: SshConnection? = null
    var listener: PlatformSocketListener? = null
    val writes = mutableListOf<ByteArray>()
    var closed = false

    override fun connect(
        connection: SshConnection,
        listener: PlatformSocketListener,
    ) {
        this.connection = connection
        this.listener = listener
    }

    override fun send(bytes: ByteArray) {
        check(!closed)
        writes += bytes.copyOf()
    }

    fun writeText(index: Int = writes.lastIndex): String = writes[index].decodeToString()

    override fun close() {
        closed = true
    }

    fun connected() {
        listener?.onConnected()
    }

    fun receive(vararg lines: String) {
        val bytes = lines.joinToString(separator = "\n", postfix = "\n").encodeToByteArray()
        listener?.onBytes(bytes)
    }

    fun receiveReadinessTreeReply(requestId: Long = 1) {
        receive("""{"ok":true,"folders":[],"chats":[],"_xd_request":$requestId}""")
    }

    fun fail(
        kind: SocketFailureKind = SocketFailureKind.IO,
        message: String = "socket failed",
        hostKey: SshHostKey? = null,
    ) {
        listener?.onClosed(SocketFailure(kind, message, hostKey))
    }
}
