package com.restartfu.xd.net

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

class CallQueueTest {
    @Test
    fun matchesRepliesByPosition() = runTest {
        val writes = mutableListOf<String>()
        val queue = CallQueue(write = { writes += it.decodeToString() })
        val first = async { queue.call(request("first")) }
        val second = async { queue.call(request("second")) }
        testScheduler.runCurrent()

        queue.acceptReply(reply("one"))
        queue.acceptReply(reply("two"))

        assertEquals(listOf("{\"op\":\"first\"}\n", "{\"op\":\"second\"}\n"), writes)
        assertEquals("one", first.await()["value"].toString().trim('"'))
        assertEquals("two", second.await()["value"].toString().trim('"'))
    }

    @Test
    fun cancellationStillConsumesItsReply() = runTest {
        val queue = CallQueue(write = {})
        val cancelled = async { queue.call(request("cancelled")) }
        val live = async { queue.call(request("live")) }
        testScheduler.runCurrent()

        cancelled.cancel(CancellationException("caller left"))
        assertTrue(cancelled.isCancelled)
        assertEquals(2, queue.size)

        queue.acceptReply(reply("discard me"))
        assertFalse(live.isCompleted)
        queue.acceptReply(reply("belongs to live"))

        assertEquals("belongs to live", live.await()["value"].toString().trim('"'))
        assertEquals(0, queue.size)
    }

    @Test
    fun dropsReplyWhenNoCallCanAnswerIt() {
        var dropped = false
        val queue = CallQueue(write = {}, onUnanswerableReply = { dropped = true })

        queue.acceptReply(reply("orphan"))

        assertTrue(dropped)
    }

    private fun request(op: String) = buildJsonObject {
        put("op", op)
    }

    private fun reply(value: String) = buildJsonObject {
        put("ok", true)
        put("value", value)
    }
}
