package com.restartfu.xd.voice

/**
 * Extracts complete, explicit speech blocks from streamed assistant text.
 *
 * Ordinary assistant text is ignored. A block is emitted only after its exact
 * closing tag arrives, so a reconnect or an interrupted turn cannot make a
 * partial response audible. Fenced and inline code are ignored while looking
 * for an opening tag, and nested speech blocks invalidate the outer block.
 */
public class SpeakTagParser {
    private enum class Mode {
        OUTSIDE,
        SPEAK,
        INVALID,
    }

    private var mode = Mode.OUTSIDE
    private var inFence = false
    private var inInlineCode = false
    private var pending = ""
    private val content = StringBuilder()

    /** Returns the complete non-blank speech blocks found in [chunk]. */
    public fun feed(chunk: String): List<String> {
        if (chunk.isEmpty()) return emptyList()
        pending += chunk
        val output = mutableListOf<String>()

        while (true) {
            when (mode) {
                Mode.OUTSIDE -> {
                    scanOutside()
                    if (mode == Mode.OUTSIDE) break
                }
                Mode.INVALID -> {
                    val close = pending.indexOf(CLOSE_TAG)
                    if (close < 0) {
                        pending = keepSuffix(pending, CLOSE_TAG)
                        break
                    }
                    pending = pending.drop(close + CLOSE_TAG.length)
                    mode = Mode.OUTSIDE
                }

                Mode.SPEAK -> {
                    val close = pending.indexOf(CLOSE_TAG)
                    val nested = pending.indexOf(OPEN_TAG)
                    if (nested >= 0 && (close < 0 || nested < close)) {
                        // A nested block is not an unambiguous speech request.
                        pending = pending.drop(nested + OPEN_TAG.length)
                        content.clear()
                        mode = Mode.INVALID
                        continue
                    }
                    if (close < 0) {
                        val keep = maxOf(
                            suffixPrefixLength(pending, CLOSE_TAG),
                            suffixPrefixLength(pending, OPEN_TAG),
                        )
                        val safe = pending.length - keep
                        if (safe > 0) content.append(pending, 0, safe)
                        pending = pending.drop(safe)
                        break
                    }

                    if (close > 0) content.append(pending, 0, close)
                    pending = pending.drop(close + CLOSE_TAG.length)
                    content.toString().trim().takeIf(String::isNotEmpty)?.let(output::add)
                    content.clear()
                    mode = Mode.OUTSIDE
                }
            }
        }

        return output
    }

    /** Discards an unfinished block and all parser state for the next turn. */
    public fun reset() {
        mode = Mode.OUTSIDE
        inFence = false
        inInlineCode = false
        pending = ""
        content.clear()
    }

    /** An unfinished block is deliberately never spoken. */
    public fun finish(): List<String> {
        reset()
        return emptyList()
    }

    private fun scanOutside() {
        while (true) {
            if (inFence) {
                val close = pending.indexOf(FENCE)
                if (close < 0) {
                    pending = keepSuffix(pending, FENCE)
                    return
                }
                pending = pending.drop(close + FENCE.length)
                inFence = false
                continue
            }

            if (inInlineCode) {
                val close = pending.indexOf('`')
                if (close < 0) {
                    pending = ""
                    return
                }
                pending = pending.drop(close + 1)
                inInlineCode = false
                continue
            }

            val open = pending.indexOf(OPEN_TAG)
            val fence = pending.indexOf(FENCE)
            val tick = pending.indexOf('`')
            val next = listOf(open, fence, tick)
                .filter { it >= 0 }
                .minOrNull()

            if (next == null) {
                val keep = maxOf(
                    suffixPrefixLength(pending, OPEN_TAG),
                    suffixPrefixLength(pending, FENCE),
                )
                pending = if (keep == 0) "" else pending.takeLast(keep)
                return
            }

            when (next) {
                open -> {
                    pending = pending.drop(open + OPEN_TAG.length)
                    content.clear()
                    mode = Mode.SPEAK
                    return
                }

                fence -> {
                    pending = pending.drop(fence + FENCE.length)
                    inFence = true
                }

                else -> {
                    // A lone backtick at the end may become a fenced marker in
                    // the next chunk. Keep it until the stream disambiguates it.
                    if (pending.length - tick < FENCE.length) {
                        pending = pending.drop(tick)
                        return
                    }
                    pending = pending.drop(tick + 1)
                    inInlineCode = true
                }
            }
        }
    }

    private fun keepSuffix(value: String, token: String): String {
        val keep = suffixPrefixLength(value, token)
        return value.takeLast(keep)
    }

    private fun suffixPrefixLength(value: String, token: String): Int {
        val maximum = minOf(value.length, token.length - 1)
        for (length in maximum downTo 1) {
            if (value.endsWith(token.take(length))) return length
        }
        return 0
    }

    private companion object {
        const val OPEN_TAG = "<speak>"
        const val CLOSE_TAG = "</speak>"
        const val FENCE = "```"
    }
}
