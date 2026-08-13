package com.restartfu.xd.store

import com.restartfu.xd.model.ChatState
import com.restartfu.xd.model.TranscriptItem
import com.restartfu.xd.model.TranscriptKind
import com.restartfu.xd.model.TodoStatus
import com.restartfu.xd.protocol.ChatReply
import com.restartfu.xd.protocol.LiveItemReply
import com.restartfu.xd.protocol.MessageReply
import com.restartfu.xd.protocol.MessagesReply
import com.restartfu.xd.protocol.PngAttachment
import com.restartfu.xd.protocol.WorktreeReply
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
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
            listOf(TranscriptEffect.Refetch),
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
            liveItems = listOf(
                TranscriptItem(
                    id = "live-tool",
                    kind = TranscriptKind.TOOL,
                    text = "Bash",
                    live = true,
                ),
            ),
            liveSegment = "partial",
        )
        val result = reduce(before, TranscriptInput.TurnFinished())

        assertEquals(false, result.state.working)
        assertEquals("", result.state.liveSegment)
        assertTrue(result.state.liveItems.isEmpty())
        assertEquals(
            listOf(TranscriptEffect.Refetch),
            result.effects,
        )
    }

    @Test
    fun changedRefetchesSettingsWhileWorking() {
        assertEquals(
            listOf(TranscriptEffect.Refetch),
            reduce(ChatState("chat", working = true), TranscriptInput.Changed).effects,
        )
        assertEquals(
            listOf(TranscriptEffect.Refetch),
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
    fun draftEventsAreRevisionedAndPreserveOmittedAttachments() {
        val png = PngAttachment(byteArrayOf(1, 2, 3))
        var state = ChatState(
            "chat",
            draft = "old",
            draftRevision = 2,
            draftAttachments = listOf(png),
        )

        state = reduce(state, TranscriptInput.Draft("new", 3)).state
        assertEquals("new", state.draft)
        assertEquals(listOf(png), state.draftAttachments)

        state = reduce(state, TranscriptInput.Draft("stale", 2, emptyList())).state
        assertEquals("new", state.draft)
        assertEquals(listOf(png), state.draftAttachments)
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
    fun todoSnapshotsStayOutOfTheTranscriptAndUpdateLive() {
        val marker = "todo_list\n" +
            "[{\"id\":\"1\",\"text\":\"Build pane\",\"status\":\"in_progress\"}]"
        var state = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat = chat(),
                messages = messages(
                    MessageReply("assistant", "Starting", 12),
                    MessageReply("tool", marker, 13),
                ),
                nowMillis = 0,
            ),
        ).state

        assertEquals(listOf("Starting"), state.messages.map { it.text })
        assertEquals("Build pane", state.todos.single().text)
        assertEquals(TodoStatus.IN_PROGRESS, state.todos.single().status)

        state = reduce(
            state,
            TranscriptInput.Tool(
                "todo_list\n" +
                    "[{\"id\":\"1\",\"text\":\"Build pane\",\"status\":\"completed\"}]",
            ),
        ).state
        assertTrue(state.liveItems.isEmpty())
        assertEquals(TodoStatus.COMPLETED, state.todos.single().status)
    }

    @Test
    fun worktreeControlsFollowHostCapability() {
        val worktrees = listOf(
            WorktreeReply(
                path = "/repo",
                branch = "main",
                detached = false,
                main = true,
                current = true,
            ),
        )
        val unavailable = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(chat(), messages(), nowMillis = 0),
        ).state
        val available = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat(worktrees = worktrees),
                messages(),
                nowMillis = 0,
            ),
        ).state
        val locked = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat(worktrees = worktrees, hasMessages = true),
                messages(),
                nowMillis = 0,
            ),
        ).state

        assertFalse(unavailable.canCreateWorktree)
        assertTrue(available.canCreateWorktree)
        assertFalse(locked.canCreateWorktree)
    }

    @Test
    fun loadedChatKeepsFastMode() {
        val state = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat(fast = true),
                messages(),
                nowMillis = 0,
            ),
        ).state

        assertTrue(state.fast)
    }

    @Test
    fun loadedChatCarriesPromptShortcuts() {
        val state = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat(shortcuts = listOf("Review the diff", "Run tests")),
                messages(),
                nowMillis = 0,
            ),
        ).state

        assertEquals(listOf("Review the diff", "Run tests"), state.shortcuts)
    }

    @Test
    fun durationRowsRemainTranscriptMetadata() {
        val result = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat = chat(),
                messages = messages(
                    MessageReply("user", "ask", 12),
                    MessageReply("assistant", "answer", 13),
                    MessageReply("duration", "7", 14),
                ),
                nowMillis = 0,
            ),
        ).state

        assertEquals(listOf("ask", "answer"), result.messages.map { it.text })
        assertEquals(false, result.hasOlderMessages)
    }

    @Test
    fun messagesExposeAndClearOlderPageAvailability() {
        val partial = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat = chat(),
                messages = messagesWithTotal(
                    2,
                    MessageReply("assistant", "newest", 13),
                ),
                nowMillis = 0,
            ),
        ).state

        assertTrue(partial.hasOlderMessages)

        val complete = reduce(
            partial.copy(loadingOlder = true),
            TranscriptInput.MessagesLoaded(
                messages(
                    MessageReply("user", "oldest", 12),
                    MessageReply("assistant", "newest", 13),
                ),
            ),
        ).state

        assertEquals(listOf("oldest", "newest"), complete.messages.map { it.text })
        assertEquals(false, complete.hasOlderMessages)
        assertEquals(false, complete.loadingOlder)
    }

    @Test
    fun messagesReloadPreservesANewerMutationError() {
        val result = reduce(
            ChatState("chat", error = "queue rejected", loadingOlder = true),
            TranscriptInput.MessagesLoaded(messages()),
        ).state

        assertEquals("queue rejected", result.error)
        assertEquals(false, result.loadingOlder)
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
            listOf(TranscriptEffect.Refetch),
            reduce(pending, TranscriptInput.PendingTimedOut("new")).effects,
        )
    }

    @Test
    fun snapshotCoveredDeltaIsNotAppliedTwice() {
        // The host writes replies and events through separate paths, so a
        // delta already folded into `segment` can still arrive afterwards.
        val loaded = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat = chat(
                    working = true,
                    segment = "hello world",
                    turnId = 7,
                    turnSequence = 4,
                ),
                messages = messages(),
                nowMillis = 0,
            ),
        ).state

        val replayed = reduce(
            loaded,
            TranscriptInput.Text(" world", turnId = 7, turnSequence = 4),
        ).state

        assertEquals("hello world", replayed.liveSegment)
    }

    @Test
    fun deltaBeyondTheSnapshotStillApplies() {
        val loaded = reduce(
            ChatState("chat"),
            TranscriptInput.Loaded(
                chat = chat(
                    working = true,
                    segment = "hello",
                    turnId = 7,
                    turnSequence = 4,
                ),
                messages = messages(),
                nowMillis = 0,
            ),
        ).state

        val advanced = reduce(
            loaded,
            TranscriptInput.Text(" world", turnId = 7, turnSequence = 5),
        ).state

        assertEquals("hello world", advanced.liveSegment)
        assertEquals(5, advanced.turnSequence)
    }

    @Test
    fun deltaFromAnOlderTurnIsDropped() {
        val state = ChatState("chat", working = true, turnId = 9, turnSequence = 2)

        val stale = reduce(
            state,
            TranscriptInput.Text("late", turnId = 8, turnSequence = 99),
        ).state

        assertEquals("", stale.liveSegment)
    }

    @Test
    fun untaggedDeltaAlwaysApplies() {
        // An older host sends no turn ids; nothing may be silently dropped.
        val state = ChatState("chat", working = true, turnId = 9, turnSequence = 2)

        val applied = reduce(state, TranscriptInput.Text("live")).state

        assertEquals("live", applied.liveSegment)
    }

    @Test
    fun staleTurnFinishedDoesNotEndTheCurrentTurn() {
        val state = ChatState("chat", working = true, turnId = 9, turnSequence = 2)

        val result = reduce(state, TranscriptInput.TurnFinished(turnId = 8))

        assertTrue(result.state.working)
        assertTrue(result.effects.isEmpty())
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
        turnId: Long? = null,
        turnSequence: Long? = null,
        worktrees: List<WorktreeReply> = emptyList(),
        hasMessages: Boolean = false,
        fast: Boolean = false,
        shortcuts: List<String> = emptyList(),
    ): ChatReply = ChatReply(
        ok = true,
        title = "Title",
        backend = "codex",
        plan = false,
        fast = fast,
        shortcuts = shortcuts,
        working = working,
        items = items,
        segment = segment,
        workingFor = workingFor,
        turnId = turnId,
        turnSequence = turnSequence,
        newWorktree = false,
        hasMessages = hasMessages,
        worktrees = worktrees,
    )

    private fun messages(vararg rows: MessageReply): MessagesReply = MessagesReply(
        ok = true,
        totalMessages = rows.size,
        lastMessageId = rows.size.toLong(),
        messages = rows.toList(),
    )

    private fun messagesWithTotal(
        total: Int,
        vararg rows: MessageReply,
    ): MessagesReply = MessagesReply(
        ok = true,
        totalMessages = total,
        lastMessageId = total.toLong(),
        messages = rows.toList(),
    )
}
