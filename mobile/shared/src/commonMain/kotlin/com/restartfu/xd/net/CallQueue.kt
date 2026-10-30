package com.restartfu.xd.net

import com.restartfu.xd.protocol.REQUEST_ID
import kotlinx.coroutines.CompletableDeferred
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.longOrNull

/**
 * Multiplexed request/reply queue.
 *
 * Every request carries a private [REQUEST_ID] the host echoes on its reply,
 * so replies are matched by id rather than by arrival position. That is what
 * keeps a `cancel` from waiting behind a slow `diff-read`.
 *
 * A host that does not echo the id still answers strictly in order, so
 * [order] is retained as a compatibility path. Abandoned ids stay in [order]
 * precisely so that path stays aligned after a caller times out.
 *
 * All methods are called by ConnectionActor's single coroutine.
 */
internal class CallQueue(
    private val write: (ByteArray) -> Unit,
    private val onUnanswerableReply: (JsonObject) -> Unit = {},
) {
    private val pending = mutableMapOf<Long, CompletableDeferred<SequencedReply>>()
    private val order = ArrayDeque<Long>()
    private val abandoned = mutableSetOf<Long>()
    private var nextId = 0L

    val size: Int
        get() = pending.size

    /** True once enough late replies are outstanding to distrust the stream. */
    val abandonedOverflow: Boolean
        get() = abandoned.size > MAX_ABANDONED_REQUESTS

    fun enqueue(
        request: JsonObject,
        response: CompletableDeferred<SequencedReply> = CompletableDeferred(),
    ): OutstandingCall {
        val id = ++nextId
        val body = JsonObject(request + (REQUEST_ID to JsonPrimitive(id)))
        pending[id] = response
        order.addLast(id)
        try {
            write("$body\n".encodeToByteArray())
        } catch (error: Throwable) {
            pending.remove(id)
            order.removeLast()
            response.completeExceptionally(error)
            throw error
        }
        return OutstandingCall(id, response)
    }

    suspend fun call(request: JsonObject): SequencedReply =
        enqueue(request).response.await()

    /**
     * Matches [reply] to its caller and returns the id it answered.
     *
     * Returns null when the reply was abandoned by a timed-out caller or has
     * no matching request at all.
     */
    fun acceptReply(reply: SequencedReply): Long? {
        val echoed = (reply.value[REQUEST_ID] as? JsonPrimitive)?.longOrNull
        val id = if (echoed != null) {
            order.remove(echoed)
            echoed
        } else {
            order.removeFirstOrNull() ?: run {
                onUnanswerableReply(reply.value)
                return null
            }
        }

        val ignored = abandoned.remove(id)
        val response = pending.remove(id)
        if (ignored) return null
        if (response == null) {
            onUnanswerableReply(reply.value)
            return null
        }
        response.complete(reply)
        return id
    }

    /**
     * Retires a timed-out caller while keeping its reply slot reserved.
     *
     * Returns the waiting deferred so the caller can be failed, or null when
     * the reply already arrived.
     */
    fun abandon(id: Long): CompletableDeferred<SequencedReply>? {
        val response = pending.remove(id) ?: return null
        abandoned += id
        return response
    }

    fun failAll(error: Throwable) {
        val waiting = pending.values.toList()
        pending.clear()
        order.clear()
        abandoned.clear()
        waiting.forEach { it.completeExceptionally(error) }
    }

    private companion object {
        const val MAX_ABANDONED_REQUESTS = 256
    }
}

internal data class OutstandingCall(
    val id: Long,
    val response: CompletableDeferred<SequencedReply>,
)
