package com.restartfu.xd.mobile.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async

/*
 * What a finished reply is read into before it is drawn, kept outside
 * composition. A lazy row forgets everything it remembered once it scrolls
 * out of view, so without this a reply was parsed and its code coloured again
 * on the main thread every time it came back.
 */

/** Where prepared values are worked out: never the main thread. */
private val PreparingScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

/**
 * The newest values of [compute], by key: at most [capacity] of them, and at
 * most [maxWeight] in all by [weigh] (the characters a key's text holds), so
 * a few very long replies cannot hold as much as hundreds of short ones. A
 * value missing is worked out once in the background however many ask for
 * it meanwhile, and kept; the ones used longest ago make room when it is full.
 */
internal class PreparedCache<K : Any, V : Any>(
    private val capacity: Int,
    private val maxWeight: Long = Long.MAX_VALUE,
    private val weigh: (K) -> Int = { 1 },
    private val compute: (K) -> V,
) {
    private val lock = Any()
    private val done = LinkedHashMap<K, V>(16, 0.75f, true)
    private var weight = 0L
    private val pending = HashMap<K, Deferred<V>>()

    /** How many times a value has been worked out, for tests. */
    val computed = AtomicInteger()

    /** How many values are kept, and their weight in all, for tests. */
    val size: Int get() = synchronized(lock) { done.size }
    val totalWeight: Long get() = synchronized(lock) { weight }

    /** Keeps [value], then lets go of the least recently used until both limits hold. Call holding [lock]. */
    private fun keep(key: K, value: V) {
        done.put(key, value)?.let { weight -= weigh(key) }
        weight += weigh(key)
        val oldest = done.entries.iterator()
        // The newest stays even when it alone is over the weight: it is what was asked for.
        while ((done.size > capacity || weight > maxWeight) && done.size > 1 && oldest.hasNext()) {
            val eldest = oldest.next()
            weight -= weigh(eldest.key)
            oldest.remove()
        }
    }

    /** The value for [key] if it is ready, without working anything out. */
    fun peek(key: K): V? = synchronized(lock) { done[key] }

    /** The value for [key]: ready at once if it is kept, otherwise worked out in the background. */
    fun request(key: K): Deferred<V> = synchronized(lock) {
        done[key]?.let { return CompletableDeferred(it) }
        pending[key]?.let { return it }
        val job = PreparingScope.async(start = CoroutineStart.LAZY) {
            computed.incrementAndGet()
            try {
                compute(key).also { value -> synchronized(lock) { keep(key, value) } }
            } finally {
                synchronized(lock) { pending.remove(key) }
            }
        }
        pending[key] = job
        job.start()
        job
    }

    /** Keeps [value] for [key], worked out elsewhere, unless one is kept already. */
    fun offer(key: K, value: V) = synchronized(lock) {
        if (done[key] == null) keep(key, value)
    }

    fun clear() = synchronized(lock) {
        done.clear()
        weight = 0
    }
}

/**
 * The value [cache] holds for [key], or null for the moment it is still
 * being worked out away from the main thread; the caller draws something
 * plain meanwhile. A value already kept is there on the first frame.
 */
@Composable
internal fun <K : Any, V : Any> rememberPrepared(cache: PreparedCache<K, V>, key: K): V? {
    val prepared = remember(cache, key) { mutableStateOf(cache.peek(key)) }
    LaunchedEffect(cache, key) {
        if (prepared.value == null) {
            prepared.value = try {
                cache.request(key).await()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Throwable) {
                // Drawn plain, as it would be by then anyway.
                null
            }
        }
    }
    return prepared.value
}
