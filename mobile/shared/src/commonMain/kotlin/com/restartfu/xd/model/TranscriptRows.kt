package com.restartfu.xd.model

/** A chat's transcript as the rows it is drawn with. */
public data class TranscriptRows(
    /** Everything but the reply still streaming in, in order. */
    val items: List<TranscriptItem>,
    /** The same, grouped into rows. */
    val settled: List<TranscriptRow>,
    /** The question each of the reader's messages answers, by item id. */
    val answered: Map<String, String>,
    /** The reply still streaming in, drawn on its own after [settled]. */
    val segment: TranscriptRow.Single?,
) {
    /** Every row, in order. */
    val rows: List<TranscriptRow>
        get() = if (segment == null) settled else settled + segment

    /** Whether there is nothing to draw yet. */
    val isEmpty: Boolean
        get() = settled.isEmpty() && segment == null
}

/** A row's list key: unique within one transcript, and stable as the transcript grows. */
public val TranscriptRow.key: String
    get() = when (this) {
        is TranscriptRow.Single -> item.id
        // Keyed on the run's first call, which is stable as the run grows.
        is TranscriptRow.Tools -> "tools-${items.first().id}"
        is TranscriptRow.AccountSwitch -> "account-switch-${item.id}"
        is TranscriptRow.Pipeline -> "pipeline-${item.id}"
        is TranscriptRow.Subagent -> "subagent-${item.id}"
    }

/**
 * Works out a chat's rows again only when what they come from changes.
 *
 * A streamed reply changes nothing but its own text, many times a second, so
 * it is kept apart from the rest: the stored rows, the message being sent and
 * the turn's finished rows are grouped once, and reading each tool row's
 * record is remembered by row. Rows that come out the same as before are the
 * same objects as before, so a list drawing them can skip them.
 *
 * Not thread-safe: one cache belongs to one chat screen.
 */
public class TranscriptRowCache {
    private var messages: List<TranscriptItem>? = null
    private var pendingUser: TranscriptItem? = null
    private var liveItems: List<TranscriptItem>? = null
    private var items: List<TranscriptItem> = emptyList()
    private var settled: List<TranscriptRow> = emptyList()
    private var answered: Map<String, String> = emptyMap()
    private var shapes: Map<String, Pair<String, ToolShape>> = emptyMap()

    public fun rows(state: ChatState): TranscriptRows {
        if (
            state.messages !== messages ||
            state.pendingUser !== pendingUser ||
            state.liveItems !== liveItems
        ) {
            settle(state)
        }
        // Its id is its own: no stored or finished row is named like it.
        val segment = state.segmentItem?.let(TranscriptRow::Single)
        return TranscriptRows(items, settled, answered, segment)
    }

    private fun settle(state: ChatState) {
        messages = state.messages
        pendingUser = state.pendingUser
        liveItems = state.liveItems
        val items = buildList {
            addAll(state.messages)
            state.pendingUser?.let(::add)
            addAll(state.liveItems)
        }
        this.items = items
        val known = shapes
        val next = HashMap<String, Pair<String, ToolShape>>(items.size)
        val grouped = ToolGrouping.rows(items) { item ->
            val cached = known[item.id]?.takeIf { (text, _) -> text == item.text }
            val shape = cached?.second ?: ToolGrouping.classify(item)
            next[item.id] = item.text to shape
            shape
        }
        shapes = next
        val previous = settled.associateBy { it.key }
        val keys = HashSet<String>(grouped.size)
        settled = grouped.map { row ->
            // A row and its stored copy can briefly both be on hand while a
            // reload catches up with a turn; lists need their keys unique.
            val unique = if (keys.add(row.key)) row else row.renamed(keys)
            previous[unique.key]?.takeIf { it == unique } ?: unique
        }
        answered = AskBlock.answered(items)
    }
}

/** The same row under an id not yet in [keys], which then holds it. */
private fun TranscriptRow.renamed(keys: MutableSet<String>): TranscriptRow {
    var suffix = 2
    while (true) {
        val renamed = when (this) {
            is TranscriptRow.Single -> copy(item = item.copy(id = "${item.id}#$suffix"))
            is TranscriptRow.Tools -> copy(
                items = listOf(items.first().let { it.copy(id = "${it.id}#$suffix") }) + items.drop(1),
            )
            is TranscriptRow.AccountSwitch -> copy(item = item.copy(id = "${item.id}#$suffix"))
            is TranscriptRow.Pipeline -> copy(item = item.copy(id = "${item.id}#$suffix"))
            is TranscriptRow.Subagent -> copy(item = item.copy(id = "${item.id}#$suffix"))
        }
        if (keys.add(renamed.key)) return renamed
        suffix++
    }
}
