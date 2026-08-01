package com.restartfu.xd.model

/** A transcript entry as it should be drawn. */
public sealed interface TranscriptRow {
    /** Anything that stands on its own, including a lone tool call. */
    public data class Single(val item: TranscriptItem) : TranscriptRow

    /** A run of tool calls worth hiding behind one line. */
    public data class Tools(val items: List<TranscriptItem>) : TranscriptRow {
        val label: String get() = "${items.size} tool calls"
    }
}

/**
 * Groups contiguous tool calls so a run collapses but a lone one does not.
 *
 * Collapsing a single call hides its command and shows a count instead, which
 * is less information in the same space. A run is where a transcript actually
 * gets buried, so that is what folds away.
 *
 * The same rule is in `Xd::UI::ToolCallGroup` on the desktop.
 */
public object ToolGrouping {
    public fun rows(items: List<TranscriptItem>): List<TranscriptRow> {
        val rows = mutableListOf<TranscriptRow>()
        var run = mutableListOf<TranscriptItem>()

        fun flush() {
            when (run.size) {
                0 -> Unit
                1 -> rows += TranscriptRow.Single(run.single())
                else -> rows += TranscriptRow.Tools(run.toList())
            }
            run = mutableListOf()
        }

        items.forEach { item ->
            if (item.kind == TranscriptKind.TOOL) {
                run += item
            } else {
                flush()
                rows += TranscriptRow.Single(item)
            }
        }
        flush()
        return rows
    }
}
