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
