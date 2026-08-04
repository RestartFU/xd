package com.restartfu.xd.voice

/**
 * Microphone capture, which is the one part of voice input that cannot move to
 * the daemon.
 *
 * A plain interface rather than an `expect class`, matching how the module
 * already handles sockets and credentials: the platform supplies one and the
 * shared code never learns which.
 */
public interface VoiceRecorder {
    /**
     * Captures 16 kHz mono 16-bit little-endian PCM until [stop] or [cancel].
     * [onChunk] receives ordered incremental samples for live recognition;
     * the return value remains the complete recording for the final pass.
     *
     * Stops on its own at [Wav.MAX_PCM_BYTES] rather than recording forever.
     */
    public suspend fun record(onChunk: (ByteArray) -> Unit): ByteArray

    /** Ends the recording; [record] returns what it has. */
    public fun stop()

    /** Ends the recording and throws it away. */
    public fun cancel()
}

/** Microphone capture failed, with a message worth showing. */
public class VoiceRecorderException(
    message: String,
    cause: Throwable? = null,
) : Exception(message, cause)
