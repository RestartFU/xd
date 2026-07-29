package com.restartfu.xd.store

import com.restartfu.xd.model.ChatState
import com.restartfu.xd.model.TranscriptKind
import com.restartfu.xd.protocol.ChatReply
import com.restartfu.xd.protocol.LiveItemReply
import com.restartfu.xd.protocol.MessageReply
import com.restartfu.xd.protocol.MessagesReply
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class TranscriptMachineTest {
    @Test
    fun turnStartMarksWorkingClearsPendingAndRefetchesMessages() {
        val state = reduce(
            ChatState("chat").let {
                TranscriptMachine.reduce(
                    it,
                    TranscriptInput.OptimisticSend("pending", "hello", 10),
                ).state
            },
            TranscriptInput.TurnStarted("Thinking", 20),
        )

        assertTrue(state.state.working)
        assertEquals("Thinking", state.state.label)
        assertEquals(20, state.state.startedAtMillis)
        assertNull(state.state.pendingUser)
        assertEquals(
            listOf(TranscriptEffect.Refetch(RefetchTarget.MESSAGES)),
            state.effects,
        )
    }

    @Test
    fun textAppendsAndToolClosesSegmentBeforeTool() {
        var state = ChatState("chat", working = true)
        state = reduce(state, TranscriptInput.Text("hello ")).state
        state = reduce(state, TranscriptInput.Text("world")).state
        state = reduce(state, TranscriptInput.Tool("Read")).state

        assertEquals("", state.liveSegment)
        assertEquals(
            listOf(TranscriptKind.ASSISTANT, TranscriptKind.TOOL),
            state.liveItems.map { it.kind },
        )
        assertEquals(listOf("hello world", "Read"), state.liveItems.map { it.text })
    }

    @Test
    fun turnFinishDiscardsLiveStateAndRefetches() {
        val before = ChatState(
            chatId = "chat",
            working = true,
            liveSegment = "partial",
        )
        val result = reduce(before, TranscriptInput.TurnFinished)

        assertEquals(false, result.state.working)
        assertEquals("", result.state.liveSegment)
        assertEquals(
            listOf(TranscriptEffect.Refetch(RefetchTarget.MESSAGES)),
            result.effects,
        )
    }

    @Test
    fun changedDoesNotRefetchWhileWorking() {
        assertTrue(
            reduce(ChatState("chat", working = true), TranscriptInput.Changed)
                .effects.isEmpty(),
        )
        assertEquals(
            listOf(TranscriptEffect.Refetch(RefetchTarget.MESSAGES)),
            reduce(ChatState("chat"), TranscriptInput.Changed).effects,
        )
    }

    @Test
    fun queuedReplacesWholeQueueAndClearsPending() {
        var state = reduce(
            ChatState("chat", queue = listOf("old")),
            TranscriptInput.OptimisticSend("pending", "message", 0),
        ).state
        state = reduce(state, TranscriptInput.Queued(listOf("one", "two"))).state

        assertEquals(listOf("one", "two"), state.queue)
        assertNull(state.pendingUser)
    }

    @Test
    fun loadedWorkingReplaysItemsAndSeedsSegment() {
        val result = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat = chat(
                    working = true,
                    items = listOf(
                        LiveItemReply(tool = false, text = "answer"),
                        LiveItemReply(tool = true, text = "Bash"),
                    ),
                    segment = "tail",
                    workingFor = 7,
                ),
                messages = messages(),
                nowMillis = 20_000,
            ),
        ).state

        assertEquals(listOf("answer", "Bash"), result.liveItems.map { it.text })
        assertEquals("tail", result.liveSegment)
        assertEquals(13_000, result.startedAtMillis)
    }

    @Test
    fun persistedMessagesMapRolesAndSecondsToMillis() {
        val result = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat = chat(),
                messages = messages(
                    MessageReply("user", "ask", 12),
                    MessageReply("assistant", "answer", 13),
                ),
                nowMillis = 0,
            ),
        ).state

        assertEquals(
            listOf(TranscriptKind.USER, TranscriptKind.ASSISTANT),
            result.messages.map { it.kind },
        )
        assertEquals(listOf(12_000L, 13_000L), result.messages.map { it.atMillis })
    }

    @Test
    fun pendingTimeoutRequestsChatAndMessagesOnlyForCurrentPendingRow() {
        val pending = reduce(
            ChatState("chat"),
            TranscriptInput.OptimisticSend("new", "hello", 1),
        ).state

        assertTrue(
            reduce(pending, TranscriptInput.PendingTimedOut("old")).effects.isEmpty(),
        )
        assertEquals(
            listOf(
                TranscriptEffect.Refetch(RefetchTarget.CHAT),
                TranscriptEffect.Refetch(RefetchTarget.MESSAGES),
            ),
            reduce(pending, TranscriptInput.PendingTimedOut("new")).effects,
        )
    }

    private fun reduce(
        state: ChatState,
        input: TranscriptInput,
    ): TranscriptTransition = TranscriptMachine.reduce(state, input)

    private fun chat(
        working: Boolean = false,
        items: List<LiveItemReply> = emptyList(),
        segment: String? = null,
        workingFor: Long? = null,
    ): ChatReply = ChatReply(
        ok = true,
        title = "Title",
        backend = "codex",
        plan = false,
        working = working,
        items = items,
        segment = segment,
        workingFor = workingFor,
        newWorktree = false,
        hasMessages = false,
    )

    private fun messages(vararg rows: MessageReply): MessagesReply = MessagesReply(
        ok = true,
        totalMessages = rows.size,
        lastMessageId = rows.size.toLong(),
        messages = rows.toList(),
    )
}
