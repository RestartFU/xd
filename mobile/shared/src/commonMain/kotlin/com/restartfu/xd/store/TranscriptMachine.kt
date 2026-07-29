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
    ) : TranscriptInput

    public data class Text(val delta: String) : TranscriptInput
    public data class Tool(val name: String) : TranscriptInput
    public data object TurnFinished : TranscriptInput
    public data object Changed : TranscriptInput
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
        is TranscriptInput.TurnStarted -> TranscriptTransition(
            state.copy(
                working = true,
                label = input.label,
                startedAtMillis = input.nowMillis,
                liveItems = emptyList(),
                liveSegment = "",
                pendingUser = null,
                error = null,
            ),
            listOf(TranscriptEffect.Refetch),
        )
        is TranscriptInput.Text -> TranscriptTransition(
            state.copy(liveSegment = state.liveSegment + input.delta),
        )
        is TranscriptInput.Tool -> {
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
                ),
            )
        }
        TranscriptInput.TurnFinished -> TranscriptTransition(
            state.copy(
                working = false,
                label = null,
                startedAtMillis = null,
                liveItems = emptyList(),
                liveSegment = "",
                pendingUser = null,
            ),
            listOf(TranscriptEffect.Refetch),
        )
        TranscriptInput.Changed -> TranscriptTransition(
            state,
            if (state.working) {
                emptyList()
            } else {
                listOf(TranscriptEffect.Refetch)
            },
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
                commands = chat.commands,
                plan = chat.plan,
                queue = chat.queue,
                working = chat.working,
                label = chat.label,
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
