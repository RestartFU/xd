package com.restartfu.xd.model

/** A question the assistant tagged for the client to render as buttons. */
public data class Ask(
    val question: String,
    val options: List<String>,
    val acceptsInput: Boolean,
)

/** The [ask] found in a reply, and the reply with every block removed. */
public data class ParsedAsk(
    val ask: Ask,
    val remainder: String,
)

/**
 * `<ask>` blocks in an assistant reply.
 *
 * The daemon stores the reply verbatim and every client parses it, so this
 * mirrors `src/xd/agent/ask.cr` line for line. Getting it wrong shows raw tags
 * to the reader rather than the buttons the assistant asked for.
 *
 * Streaming needs no equivalent of the daemon's `visible_bytes`: `text` deltas
 * are already withheld until a block completes, so a live segment never
 * contains one.
 */
public object AskBlock {
    private const val OPEN = "<ask>"
    private const val CLOSE = "</ask>"

    /** The daemon's cap. A list longer than this is not a short list. */
    public const val MAX_OPTIONS: Int = 6

    /**
     * The last valid block, and the prose with every valid block stripped.
     *
     * Older agents sometimes emit several despite the one-question contract,
     * and a raw tag must never reach Markdown rendering.
     */
    public fun parse(text: String): ParsedAsk? {
        val blocks = mutableListOf<Triple<Int, Int, Ask>>()
        var offset = 0
        while (true) {
            val open = text.indexOf(OPEN, offset).takeIf { it >= 0 } ?: break
            val candidate = parseAt(text, open)
            if (candidate == null) {
                offset = open + 1
                continue
            }
            val close = text.indexOf(CLOSE, open + OPEN.length)
            val finish = close + CLOSE.length
            blocks += Triple(open, finish, candidate)
            offset = finish
        }
        if (blocks.isEmpty()) return null

        val segments = mutableListOf<String>()
        var at = 0
        blocks.forEach { (start, finish, _) ->
            text.substring(at, start).trim().takeIf(String::isNotEmpty)?.let(segments::add)
            at = finish
        }
        text.substring(at).trim().takeIf(String::isNotEmpty)?.let(segments::add)

        return ParsedAsk(blocks.last().third, segments.joinToString("\n\n"))
    }

    /**
     * The question a chat is waiting on, or null.
     *
     * Read from the last message rather than from the `turn-finished` event
     * that announced it: the block is stored with the reply, so this survives
     * reopening the chat, where an event-only answer would not.
     *
     * A running turn, a queued message, or anything said since means the
     * question has been overtaken and its buttons would answer the wrong thing.
     */
    public fun pending(state: ChatState): Ask? {
        if (state.working || state.queue.isNotEmpty()) return null
        val last = state.visibleItems.lastOrNull() ?: return null
        if (last.kind != TranscriptKind.ASSISTANT) return null
        return parse(last.text)?.ask
    }

    private fun parseAt(text: String, open: Int): Ask? {
        val bodyStart = open + OPEN.length
        val close = text.indexOf(CLOSE, bodyStart).takeIf { it >= 0 } ?: return null

        val question = mutableListOf<String>()
        val options = mutableListOf<String>()
        var acceptsInput = false

        text.substring(bodyStart, close).lineSequence().forEach { line ->
            val stripped = line.trim()
            when {
                stripped.isEmpty() -> Unit
                stripped == "<input>" -> acceptsInput = true
                stripped.startsWith("- ") || stripped.startsWith("* ") ->
                    if (options.size < MAX_OPTIONS) options += stripped.substring(2).trim()
                // Once options begin, prose between them is not the question.
                options.isEmpty() -> question += stripped
            }
        }

        // One option is not a choice, and a block offering neither a choice nor
        // a text answer is just text.
        if (options.size < 2 && !acceptsInput) return null

        return Ask(
            question = question.takeIf { it.isNotEmpty() }?.joinToString(" ") ?: "Which one?",
            options = options.toList(),
            acceptsInput = acceptsInput,
        )
    }
}
