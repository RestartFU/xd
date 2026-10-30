package com.restartfu.xd.net

import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey

public fun interface PlatformSocketFactory {
    public fun create(): PlatformSocket
}

public interface PlatformSocket {
    public fun connect(
        connection: SshConnection,
        listener: PlatformSocketListener,
    )

    /** Sends bytes exactly as supplied. The caller includes the line feed. */
    public fun send(bytes: ByteArray)

    /** Closes this socket. Safe to call more than once. */
    public fun close()
}

public interface PlatformSocketListener {
    public fun onConnected()
    public fun onBytes(chunk: ByteArray)
    public fun onClosed(reason: SocketFailure?)
}

public enum class SocketFailureKind {
    UNREACHABLE,
    HOST_KEY_UNKNOWN,
    HOST_KEY_MISMATCH,
    AUTHENTICATION,
    IO,
    CANCELLED,
}

public data class SocketFailure(
    val kind: SocketFailureKind,
    val message: String,
    val hostKey: SshHostKey? = null,
)
