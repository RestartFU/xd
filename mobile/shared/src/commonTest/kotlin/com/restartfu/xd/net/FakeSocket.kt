package com.restartfu.xd.net

internal class FakeSocketFactory : PlatformSocketFactory {
    val sockets = mutableListOf<FakeSocket>()

    override fun create(): PlatformSocket = FakeSocket().also(sockets::add)

    val latest: FakeSocket
        get() = sockets.last()
}

internal class FakeSocket : PlatformSocket {
    var host: String? = null
    var port: Int? = null
    var pin: ByteArray? = null
    var listener: PlatformSocketListener? = null
    val writes = mutableListOf<ByteArray>()
    var closed = false

    override fun connect(
        host: String,
        port: Int,
        pinnedCertificateDer: ByteArray?,
        listener: PlatformSocketListener,
    ) {
        this.host = host
        this.port = port
        pin = pinnedCertificateDer?.copyOf()
        this.listener = listener
    }

    override fun send(bytes: ByteArray) {
        check(!closed)
        writes += bytes.copyOf()
    }

    override fun close() {
        closed = true
    }

    fun connected(certificateDer: ByteArray = byteArrayOf(1, 2, 3)) {
        listener?.onConnected(certificateDer)
    }

    fun receive(vararg lines: String) {
        val bytes = lines.joinToString(separator = "\n", postfix = "\n").encodeToByteArray()
        listener?.onBytes(bytes)
    }

    fun fail(
        kind: SocketFailureKind = SocketFailureKind.IO,
        message: String = "socket failed",
    ) {
        listener?.onClosed(SocketFailure(kind, message))
    }
}
