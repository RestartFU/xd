package com.restartfu.xd.model

/** A transcript entry as it should be drawn. */
public sealed interface TranscriptRow {
    /** Anything that stands on its own, including a lone tool call. */
    public data class Single(val item: TranscriptItem) : TranscriptRow

    /** A GitHub Actions run with live job status. */
    public data class Pipeline(
        val item: TranscriptItem,
        val run: PipelineRun,
    ) : TranscriptRow

    /** A run of ordinary tool calls worth hiding behind one line. */
    public data class Tools(val items: List<TranscriptItem>) : TranscriptRow {
        val label: String get() = "${items.size} tool calls"
    }
}

/**
 * Groups contiguous ordinary tool calls so a run collapses but a lone one
 * does not. Inline diffs always stand alone behind their own file-change row.
 *
 * Collapsing a single call hides its command and shows a count instead, which
 * is less information in the same space. A run is where a transcript actually
 * gets buried, so that is what folds away.
 *
 * The same rule is in the desktop transcript renderer.
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
            val pipeline = if (item.kind == TranscriptKind.TOOL) {
                PipelineRun.parse(item.text)
            } else {
                null
            }
            if (pipeline != null) {
                flush()
                rows += TranscriptRow.Pipeline(item, pipeline)
            } else if (item.kind == TranscriptKind.TOOL && ToolText.patch(item.text) == null) {
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
