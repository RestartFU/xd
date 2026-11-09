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

    /** A delegated agent with its own status and task details. */
    public data class Subagent(
        val item: TranscriptItem,
        val run: SubagentRun,
    ) : TranscriptRow

    /**
     * The host moved the chat to another of the assistant's accounts: a quiet
     * notice, never a tool call, so it neither joins nor hides in a run.
     */
    public data class AccountSwitch(
        val item: TranscriptItem,
        val sentence: String,
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
            val subagent = if (item.kind == TranscriptKind.TOOL) {
                SubagentRun.parse(item.text)
            } else {
                null
            }
            val switched = if (item.kind == TranscriptKind.TOOL) AccountSwitch.sentence(item.text) else null
            if (switched != null) {
                flush()
                rows += TranscriptRow.AccountSwitch(item, switched)
            } else if (pipeline != null) {
                flush()
                rows += TranscriptRow.Pipeline(item, pipeline)
            } else if (subagent != null) {
                flush()
                rows += TranscriptRow.Subagent(item, subagent)
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

/** The row the host adds when a chat moves to another account. */
public object AccountSwitch {
    private const val PREFIX = "account_switch\n"

    /** The row's one sentence, or null when [text] is not an account switch. */
    public fun sentence(text: String): String? =
        text.takeIf { it.startsWith(PREFIX) }?.removePrefix(PREFIX)?.trim()?.takeIf(String::isNotEmpty)
}
