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
 * `[image: /path.png]` on its own line in the message, so without this a sent
 * image reads as that literal text.
 *
 * The rule matches the desktop transcript parser: the whole line, and nothing
 * else on it. Prose that merely mentions `[image: ...]` mid-sentence stays
 * prose, exactly as on the desktop.
 */
public object ImageReference {
    private val MARKER = Regex("""^\[image: (.+)]$""")

    public fun parts(text: String): List<MessagePart> {
        val parts = mutableListOf<MessagePart>()
        val prose = StringBuilder()

        fun flush() {
            if (prose.isEmpty()) return
            parts += MessagePart.Prose(prose.toString())
            prose.clear()
        }

        text.split('\n').forEach { raw ->
            val line = raw.removeSuffix("\r")
            val match = MARKER.matchEntire(line)
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
}
