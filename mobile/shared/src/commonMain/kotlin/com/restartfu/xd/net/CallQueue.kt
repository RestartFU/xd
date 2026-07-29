package com.restartfu.xd.net

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Deferred
import kotlinx.serialization.json.JsonObject

/**
 * Positional request/reply queue.
 *
 * A waiting coroutine may be cancelled, but its standalone deferred and queue
 * slot remain until the matching reply arrives. Removing a cancelled slot
 * would shift every later reply onto the wrong request.
 *
 * All methods are called by ConnectionActor's single coroutine.
 */
internal class CallQueue(
    private val write: (ByteArray) -> Unit,
    private val onUnanswerableReply: (JsonObject) -> Unit = {},
) {
    private val pending = ArrayDeque<CompletableDeferred<JsonObject>>()

    val size: Int
        get() = pending.size

    fun enqueue(
        request: JsonObject,
        response: CompletableDeferred<JsonObject> = CompletableDeferred(),
    ): Deferred<JsonObject> {
        pending.addLast(response)
        try {
            write("${request}\n".encodeToByteArray())
        } catch (error: Throwable) {
            pending.removeLast()
            response.completeExceptionally(error)
            throw error
        }
        return response
    }

    suspend fun call(request: JsonObject): JsonObject = enqueue(request).await()

    fun acceptReply(reply: JsonObject) {
        val response = pending.removeFirstOrNull()
        if (response == null) {
            onUnanswerableReply(reply)
            return
        }
        response.complete(reply)
    }

    fun failAll(error: Throwable) {
        while (pending.isNotEmpty()) {
            pending.removeFirst().completeExceptionally(error)
        }
    }
}
