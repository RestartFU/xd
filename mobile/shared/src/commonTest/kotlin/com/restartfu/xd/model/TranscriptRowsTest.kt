package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertSame
import kotlin.test.assertTrue

class TranscriptRowsTest {
    private fun item(id: String, kind: TranscriptKind, text: String, messageId: Long? = null) =
        TranscriptItem(id = id, kind = kind, text = text, messageId = messageId)

    private val stored = listOf(
        item("message-1", TranscriptKind.USER, "go"),
        item("message-2", TranscriptKind.TOOL, "Read a"),
        item("message-3", TranscriptKind.TOOL, "Read b"),
        item("message-4", TranscriptKind.ASSISTANT, "done"),
    )

    @Test
    fun aStreamedWordGroupsNothingAgain() {
        val cache = TranscriptRowCache()
        val state = ChatState("chat", messages = stored, working = true, liveSegment = "Hel")
        val first = cache.rows(state)
        val second = cache.rows(state.copy(liveSegment = "Hello"))

        assertSame(first.settled, second.settled)
        assertSame(first.answered, second.answered)
        assertEquals("Hello", second.segment?.item?.text)
        assertEquals(ChatState.LIVE_SEGMENT_ID, second.rows.last().key)
        assertEquals(listOf("message-1", "tools-message-2", "message-4", ChatState.LIVE_SEGMENT_ID), second.rows.map { it.key })
    }

    @Test
    fun unchangedRowsStayTheSameObjectsWhenARowIsAdded() {
        val cache = TranscriptRowCache()
        val before = cache.rows(ChatState("chat", messages = stored))
        val after = cache.rows(
            ChatState("chat", messages = stored, liveItems = listOf(item("live-0", TranscriptKind.TOOL, "Edit", 7))),
        )

        before.settled.forEachIndexed { index, row -> assertSame(row, after.settled[index]) }
        assertEquals(before.settled.size + 1, after.settled.size)
        assertNull(after.segment)
    }

    @Test
    fun aRowOnHandTwiceStillGetsUniqueKeys() {
        val cache = TranscriptRowCache()
        val duplicate = item("message-9", TranscriptKind.ASSISTANT, "same")
        val rows = cache.rows(
            ChatState("chat", messages = stored + duplicate, liveItems = listOf(duplicate.copy())),
        ).rows
        val keys = rows.map { it.key }
        assertEquals(keys.size, keys.toSet().size)
        assertTrue("message-9" in keys)
    }

    @Test
    fun matchesGroupingTheWholeList() {
        val state = ChatState(
            "chat",
            messages = stored,
            pendingUser = item("pending-1", TranscriptKind.USER, "next"),
            liveSegment = "streaming",
        )
        assertEquals(ToolGrouping.rows(state.visibleItems), TranscriptRowCache().rows(state).rows)
    }
}
