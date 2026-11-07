package com.restartfu.xd.store

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class ProviderStatesTest {
    private data class Item(val name: String, val state: String)

    @Test
    fun aReplyDoesNotUndoAnEventThatArrivedWhileItWasInFlight() {
        val states = ProviderStates<Item> { it.name }
        assertNull(states.items.value)
        val mark = states.mark()
        // The host published "signing-in" for codex after reading the snapshot,
        // but the event reached the socket first.
        states.event(Item("codex", "signing-in"))
        states.reply(listOf(Item("codex", "signed-out"), Item("claude", "signed-in")), mark)
        assertEquals("signing-in", states.items.value!!.getValue("codex").state)
        assertEquals("signed-in", states.items.value!!.getValue("claude").state)
    }

    @Test
    fun aLaterReplyAppliesOnceNoEventIsPending() {
        val states = ProviderStates<Item> { it.name }
        states.event(Item("codex", "signing-in"))
        states.reply(listOf(Item("codex", "signed-in")), states.mark())
        assertEquals("signed-in", states.items.value!!.getValue("codex").state)
        states.clear()
        assertNull(states.items.value)
    }
}
