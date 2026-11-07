package com.restartfu.xd.store

import com.restartfu.xd.model.ChatState
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class QueueRevisionTest {
    @Test
    fun anOlderRevisionNeverReplacesANewerOne() {
        assertTrue(isOlderQueue(11, 12))
        assertFalse(isOlderQueue(12, 12))
        assertFalse(isOlderQueue(13, 12))
        // Hosts without revisions, or none seen yet, never count as older.
        assertFalse(isOlderQueue(null, 12))
        assertFalse(isOlderQueue(11, null))
    }

    @Test
    fun aLateQueuedEventDoesNotRollTheQueueBack() {
        val current = ChatState(chatId = "c", queue = listOf("new"), queueRevision = 12)
        val late = TranscriptMachine.reduce(current, TranscriptInput.Queued(listOf("old"), 11)).state
        assertEquals(listOf("new"), late.queue)
        assertEquals(12L, late.queueRevision)
        val next = TranscriptMachine.reduce(current, TranscriptInput.Queued(listOf("newer"), 13)).state
        assertEquals(listOf("newer"), next.queue)
        assertEquals(13L, next.queueRevision)
    }
}
