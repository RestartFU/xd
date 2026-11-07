package com.restartfu.xd.store

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/**
 * Per-assistant state the host both answers for and broadcasts, such as
 * sign-in or CLI updates.
 *
 * A snapshot reply and a concurrently published event can reach the socket
 * in either order, but events arrive in the order the host published them.
 * So events always apply, and a reply updates only the assistants that had
 * no event while it was in flight: for those, the event already said more.
 */
public class ProviderStates<T>(private val key: (T) -> String) {
    private data class State<T>(val items: Map<String, T>?, val events: Map<String, Long>)

    private val state = MutableStateFlow(State<T>(null, emptyMap()))
    private val _items = MutableStateFlow<Map<String, T>?>(null)

    /** Null until something has been read or broadcast. */
    public val items: StateFlow<Map<String, T>?> = _items.asStateFlow()

    /** Taken before a request, and handed back with its reply. */
    public class Mark internal constructor(internal val events: Map<String, Long>)

    public fun mark(): Mark = Mark(state.value.events)

    public fun event(item: T) {
        val name = key(item)
        state.update { current ->
            State(
                items = (current.items ?: emptyMap()) + (name to item),
                events = current.events + (name to (current.events[name] ?: 0) + 1),
            )
        }
        _items.value = state.value.items
    }

    public fun reply(items: List<T>, mark: Mark) {
        state.update { current ->
            val fresh = items.filter { current.events[key(it)] == mark.events[key(it)] }
            State((current.items ?: emptyMap()) + fresh.associateBy(key), current.events)
        }
        _items.value = state.value.items
    }

    public fun clear() {
        state.value = State(null, emptyMap())
        _items.value = null
    }
}
