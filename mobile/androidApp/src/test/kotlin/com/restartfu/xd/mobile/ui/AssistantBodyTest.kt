package com.restartfu.xd.mobile.ui

import com.restartfu.xd.markdown.Block
import com.restartfu.xd.model.AssistantSectionKind
import com.restartfu.xd.model.ChatState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A reply read as it streams in comes out the same as one read whole, with
 * what did not change handed back as the same objects; and the state the chat
 * screen reads does not move for each streamed word.
 */
class AssistantBodyTest {
    private val reply = """
        ## Plan

        Backoff starts at **2 s** and doubles.

        - read `ConnectionActor`
        - replace the fixed timer
        - reset after a good connection

        ```kotlin
        fun next(): Long = (2_000L shl attempt++).coerceAtMost(CAP)
        ```

        [image: /tmp/xd/curve.png]

        <analysis>
        The old timer never reset, so one bad night left the delay at its worst.
        </analysis>

        | attempt | delay |
        | --- | --- |
        | 1 | 2 s |
        | 6 | 64 s |

        That is everything.
    """.trimIndent()

    /** What a body draws, as plain values: parts, then each section's kind and blocks. */
    private fun List<BodyPart>.drawn(): List<Any?> = map { part ->
        when (part) {
            is BodyPart.Image -> part.path
            is BodyPart.Prose -> listOf(part.document.question) +
                part.document.sections.map { Triple(it.kind, it.text, it.blocks) }
        }
    }

    @Test fun aStreamedReplyEndsAsTheSameDocumentAsOneReadWhole() {
        // In uneven chunks, each read from where the last left off, as the screen does.
        var body: List<BodyPart>? = null
        var end = 0
        var step = 1
        while (end < reply.length) {
            end = minOf(reply.length, end + step)
            step = step % 13 + 1
            body = assistantBody(reply.substring(0, end), live = true, previous = body)
        }
        assertEquals(assistantBody(reply, live = true).drawn(), checkNotNull(body).drawn())
        // The picture, and prose on either side of it.
        assertEquals(listOf("prose", "/tmp/xd/curve.png", "prose"), body.map { (it as? BodyPart.Image)?.path ?: "prose" })
    }

    @Test fun whatDidNotChangeIsHandedBackAsTheSameObjects() {
        val cut = reply.indexOf("- replace")
        val early = assistantBody(reply.substring(0, cut), live = true)
        val later = assistantBody(reply.substring(0, cut + 30), live = true, previous = early)
        val before = (early.first() as BodyPart.Prose).document.sections.first().blocks.orEmpty()
        val after = (later.first() as BodyPart.Prose).document.sections.first().blocks.orEmpty()
        // The heading and the paragraph are finished: the same blocks, not just equal ones.
        assertTrue(before.size >= 3)
        assertSame(before[0], after[0])
        assertSame(before[1], after[1])
        // The list is still being written.
        assertNotSame(before[2], after[2])
        assertTrue(after[2] is Block.Bullets)

        // Text that did not move at all is the same part, whole.
        val again = assistantBody(reply.substring(0, cut + 30), live = true, previous = later)
        assertSame(later.first(), again.first())
    }

    @Test fun aFinishedReplysAnalysisIsOnlyParsedIfOpened() {
        val finished = assistantBody(reply, live = false)
        val sections = finished.filterIsInstance<BodyPart.Prose>().flatMap { it.document.sections }
        val analysis = sections.single { it.kind == AssistantSectionKind.ANALYSIS }
        assertNull(analysis.blocks)
        assertTrue("never reset" in analysis.text)
        assertTrue(sections.filter { it.kind == AssistantSectionKind.NORMAL }.all { it.blocks != null })
        // While the reply streams its analysis is withheld, as before: it shows once the reply is whole.
        val streaming = assistantBody(reply, live = true)
            .filterIsInstance<BodyPart.Prose>().flatMap { it.document.sections }
        assertTrue(streaming.none { it.kind == AssistantSectionKind.ANALYSIS })
        assertTrue(streaming.none { "never reset" in it.text })
    }

    @Test fun theScreensStateDoesNotMoveForEachStreamedWord() {
        val turn = ChatState(chatId = "c1", title = "Chat", working = true, turnId = 1)
        val first = turn.copy(liveSegment = "Back", turnSequence = 2).withoutLiveText()
        val second = turn.copy(liveSegment = "Backoff now starts", turnSequence = 9).withoutLiveText()
        assertEquals(first, second)
        // It still says a reply is streaming, as one row after the rest.
        assertEquals(LIVE_TEXT_ELSEWHERE, first.segmentItem?.text)
        assertEquals(ChatState.LIVE_SEGMENT_ID, first.lastVisibleItem?.id)
        // Before the first word there is no such row, and the state is untouched.
        assertSame(turn, turn.withoutLiveText())
        // With no turn running nothing is streaming: the state is the host's, whole.
        val idle = turn.copy(working = false, liveSegment = "left over", turnSequence = 4)
        assertSame(idle, idle.withoutLiveText())
    }
}
