package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs

class ToolGroupingTest {
    @Test
    fun aLoneToolCallStaysVisible() {
        val rows = ToolGrouping.rows(listOf(tool("Bash ls")))

        val single = assertIs<TranscriptRow.Single>(rows.single())
        assertEquals("Bash ls", single.item.text)
    }

    @Test
    fun aRunOfToolCallsCollapses() {
        val rows = ToolGrouping.rows(
            listOf(tool("one"), tool("two"), tool("three")),
        )

        val group = assertIs<TranscriptRow.Tools>(rows.single())
        assertEquals(3, group.items.size)
        assertEquals("3 tool calls", group.label)
    }

    @Test
    fun runsAreBrokenByAnythingElse() {
        val rows = ToolGrouping.rows(
            listOf(
                tool("a"),
                tool("b"),
                message("said something"),
                tool("c"),
            ),
        )

        assertEquals(3, rows.size)
        assertEquals(2, assertIs<TranscriptRow.Tools>(rows[0]).items.size)
        assertIs<TranscriptRow.Single>(rows[1])
        // The trailing run is one call, so it is shown rather than hidden.
        assertEquals("c", assertIs<TranscriptRow.Single>(rows[2]).item.text)
    }

    @Test
    fun anInlineDiffGetsItsOwnCollapseRow() {
        val diff = tool(
            "file_change\ndiff --git a/src/file.kt b/src/file.kt\n" +
                "--- a/src/file.kt\n+++ b/src/file.kt\n@@ -1 +1 @@\n-old\n+new",
        )
        val rows = ToolGrouping.rows(
            listOf(tool("one"), tool("two"), diff, tool("three"), tool("four")),
        )

        assertEquals(3, rows.size)
        assertEquals("2 tool calls", assertIs<TranscriptRow.Tools>(rows[0]).label)
        assertEquals(diff, assertIs<TranscriptRow.Single>(rows[1]).item)
        assertEquals("2 tool calls", assertIs<TranscriptRow.Tools>(rows[2]).label)
    }

    @Test
    fun aPipelineGetsItsOwnCardRow() {
        val pipeline = tool(
            "workflow_run\n123\n" +
                "https://github.com/RestartFU/xd/actions/runs/123",
        )
        val rows = ToolGrouping.rows(listOf(tool("before"), pipeline, tool("after")))

        assertEquals(3, rows.size)
        assertIs<TranscriptRow.Single>(rows[0])
        val card = assertIs<TranscriptRow.Pipeline>(rows[1])
        assertEquals("123", card.run.id)
        assertIs<TranscriptRow.Single>(rows[2])
    }

    @Test
    fun aSubagentGetsItsOwnCardRow() {
        val subagent = tool(
            "subagent\nthread-1\nCodex · gpt-5.6-sol\nRunning · Review the diff",
        )
        val rows = ToolGrouping.rows(listOf(tool("before"), subagent, tool("after")))

        assertEquals(3, rows.size)
        assertIs<TranscriptRow.Single>(rows[0])
        val card = assertIs<TranscriptRow.Subagent>(rows[1])
        assertEquals("thread-1", card.run.key)
        assertEquals("Review the diff", card.run.detail)
        assertIs<TranscriptRow.Single>(rows[2])
    }

    @Test
    fun nonToolItemsPassThroughInOrder() {
        val items = listOf(message("first"), message("second"))

        val rows = ToolGrouping.rows(items)

        assertEquals(
            items,
            rows.map { assertIs<TranscriptRow.Single>(it).item },
        )
    }

    @Test
    fun anEmptyTranscriptHasNoRows() {
        assertEquals(emptyList(), ToolGrouping.rows(emptyList()))
    }

    private fun tool(text: String) = TranscriptItem(
        id = "tool-$text",
        kind = TranscriptKind.TOOL,
        text = text,
    )

    private fun message(text: String) = TranscriptItem(
        id = "msg-$text",
        kind = TranscriptKind.ASSISTANT,
        text = text,
    )
}
