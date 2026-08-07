package com.restartfu.xd.net

public class LineTooLongException(
    limit: Int,
) : IllegalArgumentException("Remote line exceeds $limit bytes")

public class InvalidUtf8Exception(
    cause: Throwable,
) : IllegalArgumentException("Remote line is not valid UTF-8", cause)

/**
 * Splits raw socket bytes on LF before decoding UTF-8.
 *
 * Decoding chunks independently corrupts a multibyte sequence split across
 * reads. Keeping bytes until LF also gives one place to enforce line limits.
 */
public class LineAssembler(
    private val maxLineBytes: Int = MAX_LINE_BYTES,
) {
    private var bytes: ByteArray = ByteArray(INITIAL_CAPACITY.coerceAtMost(maxLineBytes))
    private var size: Int = 0

    init {
        require(maxLineBytes > 0) { "maxLineBytes must be positive" }
    }

    public fun append(chunk: ByteArray): List<String> {
        if (chunk.isEmpty()) return emptyList()

        val lines = mutableListOf<String>()
        var start = 0
        while (start < chunk.size) {
            var end = start
            while (end < chunk.size && chunk[end] != LF) end += 1
            val count = end - start
            if (count > maxLineBytes - size) throw LineTooLongException(maxLineBytes)
            if (count > 0) {
                ensureCapacity(size + count)
                chunk.copyInto(bytes, destinationOffset = size, startIndex = start, endIndex = end)
                size += count
            }

            if (end < chunk.size) {
                if (size > 0) {
                    lines += decodeLine()
                }
                size = 0
                start = end + 1
            } else {
                start = end
            }
        }
        return lines
    }

    public fun reset() {
        size = 0
        val initialCapacity = INITIAL_CAPACITY.coerceAtMost(maxLineBytes)
        if (bytes.size > initialCapacity) bytes = ByteArray(initialCapacity)
    }

    private fun ensureCapacity(required: Int) {
        if (required <= bytes.size) return
        val grown = (bytes.size.coerceAtLeast(1) * 2).coerceAtMost(maxLineBytes)
        bytes = bytes.copyOf(grown.coerceAtLeast(required))
    }

    private fun decodeLine(): String = try {
        bytes.decodeToString(
            startIndex = 0,
            endIndex = size,
            throwOnInvalidSequence = true,
        )
    } catch (error: Throwable) {
        throw InvalidUtf8Exception(error)
    }

    public companion object {
        /** Mirrors `daemon-rs::FRAME_LIMIT`. */
        public const val MAX_LINE_BYTES: Int = 96 * 1024 * 1024
        private const val INITIAL_CAPACITY: Int = 4096
        private const val LF: Byte = 0x0a
    }
}
