package com.restartfu.xd.net

public fun interface PlatformSocketFactory {
    public fun create(): PlatformSocket
}

public interface PlatformSocket {
    /**
     * Connects and starts delivering serialized callbacks.
     *
     * A null pin is legal only for a pairing greeting. Every normal
     * connection must compare the exact presented leaf DER with its pin.
     */
    public fun connect(
        host: String,
        port: Int,
        pinnedCertificateDer: ByteArray?,
        listener: PlatformSocketListener,
    )

    /** Sends bytes exactly as supplied. The caller includes the line feed. */
    public fun send(bytes: ByteArray)

    /** Closes this socket. Safe to call more than once. */
    public fun close()
}

public interface PlatformSocketListener {
    public fun onConnected(leafCertificateDer: ByteArray)
    public fun onBytes(chunk: ByteArray)
    public fun onClosed(reason: SocketFailure?)
}

public enum class SocketFailureKind {
    UNREACHABLE,
    PIN_MISMATCH,
    TLS,
    IO,
    CANCELLED,
}

public data class SocketFailure(
    val kind: SocketFailureKind,
    val message: String,
)
