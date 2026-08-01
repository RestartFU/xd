package com.restartfu.xd.store

import com.restartfu.xd.model.ChatState
import com.restartfu.xd.model.TranscriptItem
import com.restartfu.xd.model.TranscriptKind
import com.restartfu.xd.protocol.ChatReply
import com.restartfu.xd.protocol.MessagesReply

public sealed interface TranscriptEffect {
    public data object Refetch : TranscriptEffect
}

public sealed interface TranscriptInput {
    public data class Loaded(
        val chat: ChatReply,
        val messages: MessagesReply,
        val nowMillis: Long,
    ) : TranscriptInput

    public data class MessagesLoaded(val messages: MessagesReply) : TranscriptInput

    public data class TurnStarted(
        val label: String?,
        val nowMillis: Long,
        val turnId: Long? = null,
        val turnSequence: Long? = null,
    ) : TranscriptInput

    public data class Text(
        val delta: String,
        val turnId: Long? = null,
        val turnSequence: Long? = null,
    ) : TranscriptInput

    public data class Tool(
        val name: String,
        val turnId: Long? = null,
        val turnSequence: Long? = null,
    ) : TranscriptInput

    public data class TurnFinished(
        val turnId: Long? = null,
        val turnSequence: Long? = null,
    ) : TranscriptInput
    public data object Changed : TranscriptInput
    public data class Commands(val commands: List<String>) : TranscriptInput
    public data class Queued(val messages: List<String>) : TranscriptInput

    public data class OptimisticSend(
        val id: String,
        val text: String,
        val nowMillis: Long,
    ) : TranscriptInput

    public data class PendingTimedOut(val id: String) : TranscriptInput

    public data class SendFailed(
        val id: String,
        val message: String,
    ) : TranscriptInput

    public data class RequestFailed(val message: String) : TranscriptInput
}

public data class TranscriptTransition(
    val state: ChatState,
    val effects: List<TranscriptEffect> = emptyList(),
)

public object TranscriptMachine {
    public fun reduce(
        state: ChatState,
        input: TranscriptInput,
    ): TranscriptTransition = when (input) {
        is TranscriptInput.Loaded -> loaded(state, input)
        is TranscriptInput.MessagesLoaded -> TranscriptTransition(
            state.withMessages(input.messages).copy(
                loadingOlder = false,
            ),
        )
        is TranscriptInput.TurnStarted -> if (state.isStaleTurn(input.turnId)) {
            TranscriptTransition(state)
        } else {
            TranscriptTransition(
                state.copy(
                    working = true,
                    label = input.label,
                    turnId = input.turnId,
                    turnSequence = input.turnSequence ?: 0,
                    startedAtMillis = input.nowMillis,
                    liveItems = emptyList(),
                    liveSegment = "",
                    pendingUser = null,
                    error = null,
                ),
                listOf(TranscriptEffect.Refetch),
            )
        }
        is TranscriptInput.Text -> if (
            state.covers(input.turnId, input.turnSequence)
        ) {
            TranscriptTransition(state)
        } else {
            TranscriptTransition(
                state.copy(liveSegment = state.liveSegment + input.delta)
                    .advancedTo(input.turnId, input.turnSequence),
            )
        }
        is TranscriptInput.Tool -> if (
            state.covers(input.turnId, input.turnSequence)
        ) {
            TranscriptTransition(state)
        } else {
            val closedSegment = if (state.liveSegment.isEmpty()) {
                state.liveItems
            } else {
                state.liveItems + TranscriptItem(
                    id = "live-${state.liveItems.size}",
                    kind = TranscriptKind.ASSISTANT,
                    text = state.liveSegment,
                    live = true,
                )
            }
            TranscriptTransition(
                state.copy(
                    liveItems = closedSegment + TranscriptItem(
                        id = "tool-${closedSegment.size}",
                        kind = TranscriptKind.TOOL,
                        text = input.name,
                        live = true,
                    ),
                    liveSegment = "",
                ).advancedTo(input.turnId, input.turnSequence),
            )
        }
        is TranscriptInput.TurnFinished -> if (state.isStaleTurn(input.turnId)) {
            TranscriptTransition(state)
        } else {
            TranscriptTransition(
                state.copy(
                    working = false,
                    label = null,
                    turnId = null,
                    turnSequence = 0,
                    startedAtMillis = null,
                    liveItems = emptyList(),
                    liveSegment = "",
                    pendingUser = null,
                ),
                listOf(TranscriptEffect.Refetch),
            )
        }
        TranscriptInput.Changed -> TranscriptTransition(
            state,
            if (state.working) {
                emptyList()
            } else {
                listOf(TranscriptEffect.Refetch)
            },
        )
        is TranscriptInput.Commands -> TranscriptTransition(
            state.copy(commands = input.commands),
        )
        is TranscriptInput.Queued -> TranscriptTransition(
            state.copy(queue = input.messages, pendingUser = null),
        )
        is TranscriptInput.OptimisticSend -> TranscriptTransition(
            state.copy(
                pendingUser = TranscriptItem(
                    id = input.id,
                    kind = TranscriptKind.USER,
                    text = input.text,
                    atMillis = input.nowMillis,
                    live = true,
                ),
            ),
        )
        is TranscriptInput.PendingTimedOut -> {
            if (state.pendingUser?.id != input.id) {
                TranscriptTransition(state)
            } else {
                TranscriptTransition(
                    state.copy(pendingUser = null),
                    listOf(TranscriptEffect.Refetch),
                )
            }
        }
        is TranscriptInput.SendFailed -> TranscriptTransition(
            state.copy(
                pendingUser = if (state.pendingUser?.id == input.id) {
                    null
                } else {
                    state.pendingUser
                },
                error = input.message,
            ),
        )
        is TranscriptInput.RequestFailed -> TranscriptTransition(
            state.copy(error = input.message),
        )
    }

    /**
     * True when this state already contains the given turn event.
     *
     * The daemon writes replies and events through separate paths, so a `text`
     * or `tool` event already folded into a `chat` snapshot's `segment` can
     * still arrive after that snapshot. Arrival order alone therefore cannot
     * decide this; the turn watermark can.
     */
    private fun ChatState.covers(turnId: Long?, turnSequence: Long?): Boolean {
        if (turnId == null || turnSequence == null) return false
        val current = this.turnId ?: return false
        if (turnId != current) return turnId < current
        return turnSequence <= this.turnSequence
    }

    /** An event from a turn older than the one in hand is stale. */
    private fun ChatState.isStaleTurn(turnId: Long?): Boolean {
        if (turnId == null) return false
        val current = this.turnId ?: return false
        return turnId < current
    }

    private fun ChatState.advancedTo(
        turnId: Long?,
        turnSequence: Long?,
    ): ChatState = if (turnId == null || turnSequence == null) {
        this
    } else {
        copy(turnId = turnId, turnSequence = turnSequence)
    }

    private fun loaded(
        state: ChatState,
        input: TranscriptInput.Loaded,
    ): TranscriptTransition {
        val chat = input.chat
        val liveItems = if (chat.working) {
            chat.items.mapIndexed { index, item ->
                TranscriptItem(
                    id = "loaded-live-$index",
                    kind = if (item.tool) TranscriptKind.TOOL else TranscriptKind.ASSISTANT,
                    text = item.text,
                    live = true,
                )
            }
        } else {
            emptyList()
        }
        return TranscriptTransition(
            state.copy(
                title = chat.title,
                backend = chat.backend,
                model = chat.model,
                commands = chat.commands,
                plan = chat.plan,
                queue = chat.queue,
                working = chat.working,
                label = chat.label,
                turnId = if (chat.working) chat.turnId else null,
                turnSequence = if (chat.working) chat.turnSequence ?: 0 else 0,
                startedAtMillis = chat.workingFor?.let { input.nowMillis - it * 1000 },
                liveItems = liveItems,
                liveSegment = if (chat.working) chat.segment.orEmpty() else "",
                pendingUser = null,
                loading = false,
                loadingOlder = false,
                error = null,
            ).withMessages(input.messages),
        )
    }

    private fun ChatState.withMessages(reply: MessagesReply): ChatState {
        val visible = reply.messages.filterNot { it.role == "duration" }
        return copy(
            messages = visible.mapIndexed { index, message ->
                TranscriptItem(
                    id = "persisted-${message.at}-${visible.size - index}",
                    kind = message.role.toKind(),
                    text = message.content,
                    atMillis = message.at * 1000,
                    label = message.label,
                )
            },
            hasOlderMessages = reply.messages.size < reply.totalMessages,
        )
    }

    private fun String.toKind(): TranscriptKind = when (this) {
        "user" -> TranscriptKind.USER
        "assistant" -> TranscriptKind.ASSISTANT
        "tool" -> TranscriptKind.TOOL
        else -> TranscriptKind.SYSTEM
    }
}
