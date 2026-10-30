package com.restartfu.xd.voice

/** Removes speech wrapper tags from transcript text without touching code. */
public object SpeakTagVisibility {
    private const val OPEN_TAG = "<speak>"
    private const val CLOSE_TAG = "</speak>"
    private const val FENCE = "```"

    /**
     * Keeps the speech body visible while hiding its transport markup. A live,
     * unfinished block shows its body but never exposes a partial tag; a final
     * malformed block stays literal so the transcript is not silently changed.
     */
    public fun render(text: String, live: Boolean = false): String {
        if (text.isEmpty()) return text

        val output = StringBuilder()
        var cursor = 0
        var blockStart = 0
        var inSpeech = false

        while (cursor < text.length) {
            if (inSpeech) {
                val close = text.indexOf(CLOSE_TAG, cursor)
                val nested = text.indexOf(OPEN_TAG, cursor)
                if (nested >= 0 && (close < 0 || nested < close)) {
                    val finish = if (close >= 0) close + CLOSE_TAG.length else text.length
                    output.append(text, blockStart, finish)
                    cursor = finish
                    inSpeech = false
                    continue
                }
                if (close < 0) {
                    if (live) {
                        val keep = suffixPrefixLength(text.substring(cursor), CLOSE_TAG)
                        val safe = text.length - keep
                        if (safe > cursor) output.append(text, cursor, safe)
                    } else {
                        output.append(text, blockStart, text.length)
                    }
                    break
                }
                output.append(text, cursor, close)
                cursor = close + CLOSE_TAG.length
                inSpeech = false
                continue
            }

            val open = text.indexOf(OPEN_TAG, cursor)
            val close = text.indexOf(CLOSE_TAG, cursor)
            val fence = text.indexOf(FENCE, cursor)
            val tick = text.indexOf('`', cursor)
            val next = listOf(open, close, fence, tick)
                .filter { it >= 0 }
                .minOrNull()

            if (next == null) {
                if (live) {
                    val tail = text.substring(cursor)
                    val keep = maxOf(
                        suffixPrefixLength(tail, OPEN_TAG),
                        suffixPrefixLength(tail, CLOSE_TAG),
                    )
                    val safe = tail.length - keep
                    if (safe > 0) output.append(tail, 0, safe)
                } else {
                    output.append(text, cursor, text.length)
                }
                break
            }

            when {
                fence == next -> {
                    val finish = text.indexOf(FENCE, next + FENCE.length)
                    if (finish < 0) {
                        output.append(text, cursor, text.length)
                        break
                    }
                    output.append(text, cursor, finish + FENCE.length)
                    cursor = finish + FENCE.length
                }

                tick == next -> {
                    val finish = text.indexOf('`', next + 1)
                    if (finish < 0) {
                        output.append(text, cursor, text.length)
                        break
                    }
                    output.append(text, cursor, finish + 1)
                    cursor = finish + 1
                }

                open == next -> {
                    output.append(text, cursor, next)
                    cursor = next + OPEN_TAG.length
                    blockStart = next
                    inSpeech = true
                }

                else -> {
                    // A closing tag without its opening partner is literal.
                    output.append(text, cursor, next + CLOSE_TAG.length)
                    cursor = next + CLOSE_TAG.length
                }
            }
        }

        return output.toString()
    }

    private fun suffixPrefixLength(value: String, token: String): Int {
        val maximum = minOf(value.length, token.length - 1)
        for (length in maximum downTo 1) {
            if (value.endsWith(token.take(length))) return length
        }
        return 0
    }
}
