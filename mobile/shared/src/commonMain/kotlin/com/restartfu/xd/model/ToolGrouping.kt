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

    /**
     * The host compacted, or is compacting, the chat's context: a quiet
     * divider, never a tool call, so it neither joins nor hides in a run.
     */
    public data class Compaction(
        val item: TranscriptItem,
        val compaction: ContextCompaction,
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
    public fun rows(items: List<TranscriptItem>): List<TranscriptRow> = rows(items, ::classify)

    /** As [rows], reading each item's shape through [shapes], which may remember them. */
    public fun rows(
        items: List<TranscriptItem>,
        shapes: (TranscriptItem) -> ToolShape,
    ): List<TranscriptRow> {
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
            when (val shape = shapes(item)) {
                is ToolShape.Switch -> {
                    flush()
                    rows += TranscriptRow.AccountSwitch(item, shape.sentence)
                }
                is ToolShape.Compaction -> {
                    flush()
                    rows += TranscriptRow.Compaction(item, shape.compaction)
                }
                is ToolShape.Pipeline -> {
                    flush()
                    rows += TranscriptRow.Pipeline(item, shape.run)
                }
                is ToolShape.Subagent -> {
                    flush()
                    rows += TranscriptRow.Subagent(item, shape.run)
                }
                ToolShape.Ordinary -> run += item
                ToolShape.Alone -> {
                    flush()
                    rows += TranscriptRow.Single(item)
                }
            }
        }
        flush()
        return rows
    }

    /** What kind of row [item] makes, before runs are grouped. */
    public fun classify(item: TranscriptItem): ToolShape {
        if (item.kind != TranscriptKind.TOOL) return ToolShape.Alone
        AccountSwitch.sentence(item.text)?.let { return ToolShape.Switch(it) }
        ContextCompaction.parse(item.text)?.let { return ToolShape.Compaction(it) }
        PipelineRun.parse(item.text)?.let { return ToolShape.Pipeline(it) }
        SubagentRun.parse(item.text)?.let { return ToolShape.Subagent(it) }
        return if (ToolText.isPatch(item.text)) ToolShape.Alone else ToolShape.Ordinary
    }
}

/** How one transcript item is drawn on its own, as [ToolGrouping.classify] reads it. */
public sealed interface ToolShape {
    /** A user, assistant or system row, or an inline diff: never grouped. */
    public data object Alone : ToolShape

    /** An ordinary tool call, which joins a run of its neighbours. */
    public data object Ordinary : ToolShape
    public data class Switch(val sentence: String) : ToolShape
    public data class Compaction(val compaction: ContextCompaction) : ToolShape
    public data class Pipeline(val run: PipelineRun) : ToolShape
    public data class Subagent(val run: SubagentRun) : ToolShape
}

/** The row the host adds when a chat moves to another account. */
public object AccountSwitch {
    private const val PREFIX = "account_switch\n"

    /** The row's one sentence, or null when [text] is not an account switch. */
    public fun sentence(text: String): String? =
        text.takeIf { it.startsWith(PREFIX) }?.removePrefix(PREFIX)?.trim()?.takeIf(String::isNotEmpty)
}
