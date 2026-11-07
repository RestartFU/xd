package com.restartfu.xd.store

import kotlin.test.Test
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
}
