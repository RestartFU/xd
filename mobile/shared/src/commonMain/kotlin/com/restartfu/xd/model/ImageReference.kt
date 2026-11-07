package com.restartfu.xd.model

/** A stretch of a message: prose, or an image the host stored. */
public sealed interface MessagePart {
    public data class Prose(val text: String) : MessagePart
    public data class Image(val path: String) : MessagePart
}

/**
 * Splits a message around the image markers the host writes.
 *
 * Sending an attachment stores the PNG on the host and leaves
 * `[image: /path.png]` on its own line in the message, and an assistant shows
 * an image the same way, so without this an image reads as that literal text.
 *
 * The rule matches the desktop transcript parser: the whole line, and nothing
 * else on it. Prose that merely mentions `[image: ...]` mid-sentence stays
 * prose, exactly as on the desktop.
 *
 * A reply is Markdown, so a marker in its code fence is code. What a user sent
 * is not: their markers follow whatever they typed, fence or no fence.
 */
public object ImageReference {
    private val MARKER = Regex("""^\[image: (.+)]$""")

    public fun parts(text: String, markdown: Boolean = false): List<MessagePart> {
        val parts = mutableListOf<MessagePart>()
        val prose = StringBuilder()
        var fence: Fence? = null

        fun flush() {
            if (prose.isEmpty()) return
            parts += MessagePart.Prose(prose.toString())
            prose.clear()
        }

        text.split('\n').forEach { raw ->
            val line = raw.removeSuffix("\r")
            if (markdown) {
                val open = fence
                fence = if (open == null) Fence.opening(line) else open.takeUnless { it.closes(line) }
            }
            val match = if (fence == null) MARKER.matchEntire(line) else null
            if (match != null) {
                flush()
                parts += MessagePart.Image(match.groupValues[1])
            } else {
                if (prose.isNotEmpty()) prose.append('\n')
                prose.append(line)
            }
        }
        flush()

        // A message with no markers is one run of prose, which keeps the
        // common case free of allocation downstream.
        return parts
    }

    public fun hasImage(text: String): Boolean =
        text.split('\n').any { MARKER.matches(it.removeSuffix("\r")) }

    private data class Fence(val marker: Char, val length: Int) {
        fun closes(line: String): Boolean {
            val trimmed = line.trim()
            return trimmed.length >= length && trimmed.all { it == marker }
        }

        companion object {
            fun opening(line: String): Fence? {
                val trimmed = line.trimStart()
                if (line.length - trimmed.length > 3) return null
                val marker = trimmed.firstOrNull()?.takeIf { it == '`' || it == '~' } ?: return null
                val length = trimmed.takeWhile { it == marker }.length
                return if (length >= 3) Fence(marker, length) else null
            }
        }
    }
}
