package com.restartfu.xd.store

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * Per-assistant state the host both answers for and broadcasts, such as
 * sign-in or CLI updates.
 *
 * A snapshot reply and a concurrently published event can reach the socket
 * in either order, but events arrive in the order the host published them.
 * So events always apply, and a reply updates only the assistants that had
 * no event while it was in flight: for those, the event already said more.
 * Replies can also arrive out of order among themselves, so one never
 * replaces an assistant's state from a request newer than its own. Every
 * change and its publication happen under one lock, so what observers see
 * is always the newest state.
 */
public class ProviderStates<T>(private val key: (T) -> String) {
    private data class State<T>(
        val items: Map<String, T>?,
        val events: Map<String, Long>,
        /** The newest request whose reply set each assistant's state. */
        val applied: Map<String, Long> = emptyMap(),
        val requests: Long = 0,
        /** Advanced by [clear], so a reply for a forgotten host is refused. */
        val epoch: Long = 0,
    )

    private val lock = Mutex()
    private var state = State<T>(null, emptyMap())
    private val _items = MutableStateFlow<Map<String, T>?>(null)

    /** Null until something has been read or broadcast. */
    public val items: StateFlow<Map<String, T>?> = _items.asStateFlow()

    /** Taken before a request, and handed back with its reply. */
    public class Mark internal constructor(
        internal val events: Map<String, Long>,
        internal val request: Long,
        internal val epoch: Long,
    )

    public suspend fun mark(): Mark = lock.withLock {
        val request = state.requests + 1
        state = state.copy(requests = request)
        Mark(state.events, request, state.epoch)
    }

    public suspend fun event(item: T): Unit = lock.withLock {
        val name = key(item)
        state = state.copy(
            items = (state.items ?: emptyMap()) + (name to item),
            events = state.events + (name to (state.events[name] ?: 0) + 1),
        )
        _items.value = state.items
    }

    public suspend fun reply(items: List<T>, mark: Mark): Unit = lock.withLock {
        if (mark.epoch != state.epoch) return@withLock
        val fresh = items.filter {
            state.events[key(it)] == mark.events[key(it)] &&
                mark.request > (state.applied[key(it)] ?: 0)
        }
        state = state.copy(
            items = (state.items ?: emptyMap()) + fresh.associateBy(key),
            applied = state.applied + fresh.associate { key(it) to mark.request },
        )
        _items.value = state.items
    }

    public suspend fun clear(): Unit = lock.withLock {
        state = State(null, emptyMap(), requests = state.requests, epoch = state.epoch + 1)
        _items.value = null
    }
}
