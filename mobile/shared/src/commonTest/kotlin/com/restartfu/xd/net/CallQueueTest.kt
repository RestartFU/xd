package com.restartfu.xd.net

import com.restartfu.xd.protocol.REQUEST_ID
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

class CallQueueTest {
    @Test
    fun stampsEveryRequestWithItsOwnId() {
        val writes = mutableListOf<String>()
        val queue = CallQueue(write = { writes += it.decodeToString() })
        queue.enqueue(request("first"))
        queue.enqueue(request("second"))

        assertEquals(
            listOf(
                "{\"op\":\"first\",\"$REQUEST_ID\":1}\n",
                "{\"op\":\"second\",\"$REQUEST_ID\":2}\n",
            ),
            writes,
        )
    }

    @Test
    fun matchesRepliesByIdOutOfOrder() = runTest {
        val queue = CallQueue(write = {})
        val slow = async { queue.call(request("slow")) }
        val quick = async { queue.call(request("quick")) }
        testScheduler.runCurrent()

        // The whole point of multiplexing: the second request may be answered
        // first without stranding or misassigning the first.
        queue.acceptReply(SequencedReply(1, reply("quick answer", id = 2)))
        assertFalse(slow.isCompleted)
        assertEquals(
            "quick answer",
            quick.await().value.getValue("value").jsonPrimitive.content,
        )

        queue.acceptReply(SequencedReply(2, reply("slow answer", id = 1)))
        assertEquals(
            "slow answer",
            slow.await().value.getValue("value").jsonPrimitive.content,
        )
        assertEquals(0, queue.size)
    }

    @Test
    fun fallsBackToPositionWhenTheHostEchoesNoId() = runTest {
        val queue = CallQueue(write = {})
        val first = async { queue.call(request("first")) }
        val second = async { queue.call(request("second")) }
        testScheduler.runCurrent()

        queue.acceptReply(SequencedReply(1, reply("one")))
        queue.acceptReply(SequencedReply(2, reply("two")))

        assertEquals("one", first.await().value.getValue("value").jsonPrimitive.content)
        assertEquals("two", second.await().value.getValue("value").jsonPrimitive.content)
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

        queue.acceptReply(SequencedReply(1, reply("discard me", id = 1)))
        assertFalse(live.isCompleted)
        queue.acceptReply(SequencedReply(2, reply("belongs to live", id = 2)))

        assertEquals(
            "belongs to live",
            live.await().value.getValue("value").jsonPrimitive.content,
        )
        assertEquals(0, queue.size)
    }

    // These two drive the queue directly rather than through `async { call }`:
    // an abandoned call never completes, and a dangling async child would fail
    // the test scope instead of the assertion under test.

    @Test
    fun abandonedCallDiscardsItsLateReplyAndSparesTheNextCaller() = runTest {
        val queue = CallQueue(write = {})
        val timedOut = queue.enqueue(request("slow"))
        val later = queue.enqueue(request("later"))

        val waiting = queue.abandon(timedOut.id)
        assertEquals(timedOut.response, waiting)
        waiting?.completeExceptionally(CallTimedOutException())

        // The late reply must be dropped, not handed to `later`.
        assertNull(queue.acceptReply(SequencedReply(1, reply("too late", id = timedOut.id))))
        assertFalse(later.response.isCompleted)

        queue.acceptReply(SequencedReply(2, reply("mine", id = later.id)))
        assertEquals(
            "mine",
            later.response.await().value.getValue("value").jsonPrimitive.content,
        )
        assertTrue(
            timedOut.response.getCompletionExceptionOrNull() is CallTimedOutException,
        )
    }

    @Test
    fun abandonedPositionStillAlignsTheCompatibilityPath() = runTest {
        val queue = CallQueue(write = {})
        val timedOut = queue.enqueue(request("slow"))
        val later = queue.enqueue(request("later"))

        queue.abandon(timedOut.id)?.completeExceptionally(CallTimedOutException())

        // An id-less host answers in order, so the first reply belongs to
        // the abandoned call and must not shift onto `later`.
        assertNull(queue.acceptReply(SequencedReply(1, reply("too late"))))
        assertFalse(later.response.isCompleted)

        queue.acceptReply(SequencedReply(2, reply("mine")))
        assertEquals(
            "mine",
            later.response.await().value.getValue("value").jsonPrimitive.content,
        )
    }

    @Test
    fun dropsReplyWhenNoCallCanAnswerIt() {
        var dropped = false
        val queue = CallQueue(write = {}, onUnanswerableReply = { dropped = true })

        queue.acceptReply(SequencedReply(1, reply("orphan")))

        assertTrue(dropped)
    }

    private fun request(op: String) = buildJsonObject {
        put("op", op)
    }

    private fun reply(value: String, id: Long? = null) = buildJsonObject {
        put("ok", true)
        put("value", value)
        if (id != null) put(REQUEST_ID, id)
    }
}
