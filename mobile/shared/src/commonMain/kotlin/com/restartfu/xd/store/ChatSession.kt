package com.restartfu.xd.store

import com.restartfu.xd.model.ChatState
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.net.SequencedEvent
import com.restartfu.xd.protocol.ChatOption
import com.restartfu.xd.protocol.ChatReply
import com.restartfu.xd.protocol.MessagesReply
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.PngAttachment
import com.restartfu.xd.protocol.decodeReply
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

public class ChatSession internal constructor(
    private val core: ChatSessionCore,
    private val release: () -> Unit,
) : AutoCloseable {
    public val state: StateFlow<ChatState> = core.state
    private var closed = false

    public suspend fun send(
        text: String,
        images: List<PngAttachment> = emptyList(),
    ): Unit = core.send(text, images)

    public suspend fun cancel(): Unit = core.call(Ops.cancel(core.chatId))

    public suspend fun enqueue(text: String): Unit = core.call(Ops.queue(core.chatId, text))

    public suspend fun dropQueued(index: Int? = null): Unit =
        core.call(Ops.dropQueue(core.chatId, index))

    public suspend fun setOption(option: ChatOption, value: String): Unit =
        core.call(Ops.setOption(core.chatId, option, value))

    override fun close() {
        if (closed) return
        closed = true
        release()
    }
}

internal class ChatSessionCore(
    val chatId: String,
    private val actor: ConnectionActor,
    private val scope: CoroutineScope,
    private val nowMillis: () -> Long,
) {
    private val stateMutex = Mutex()
    private val reloadMutex = Mutex()
    private val _state = MutableStateFlow(ChatState(chatId = chatId))
    private var pendingSequence = 0L
    private var pendingTimer: Job? = null
    private var reloadInProgress = false
    private val inputsDuringReload = mutableListOf<BufferedInput>()

    val state: StateFlow<ChatState> = _state.asStateFlow()

    suspend fun reload() {
        reloadMutex.withLock {
            stateMutex.withLock {
                reloadInProgress = true
                inputsDuringReload.clear()
                _state.value = _state.value.copy(loading = true, error = null)
            }
            try {
                val chatReply = actor.callSequenced(Ops.chat(chatId))
                val chat = chatReply.value.decodeReply<ChatReply>()
                val messages = actor.call(Ops.messages(chatId)).decodeReply<MessagesReply>()
                val replayEffects = stateMutex.withLock {
                    var transition = TranscriptMachine.reduce(
                        _state.value,
                        TranscriptInput.Loaded(chat, messages, nowMillis()),
                    )
                    val effects = mutableListOf<TranscriptEffect>()
                    for (buffered in inputsDuringReload) {
                        if (buffered.sequence != null &&
                            buffered.sequence <= chatReply.sequence
                        ) {
                            continue
                        }
                        transition = TranscriptMachine.reduce(transition.state, buffered.input)
                        effects += transition.effects
                    }
                    _state.value = transition.state
                    reloadInProgress = false
                    inputsDuringReload.clear()
                    effects
                }
                if (replayEffects.isNotEmpty()) scope.launch { reload() }
            } catch (error: Throwable) {
                stateMutex.withLock {
                    reloadInProgress = false
                    inputsDuringReload.clear()
                    _state.value = _state.value.copy(
                        loading = false,
                        error = error.message ?: "Could not load the chat",
                    )
                }
            }
        }
    }

    suspend fun send(
        text: String,
        images: List<PngAttachment>,
    ) {
        require(text.isNotEmpty() || images.isNotEmpty())
        val pendingId = "pending-${++pendingSequence}"
        apply(TranscriptInput.OptimisticSend(pendingId, text, nowMillis()))
        pendingTimer?.cancel()
        pendingTimer = scope.launch {
            delay(PENDING_TIMEOUT_MILLIS)
            apply(TranscriptInput.PendingTimedOut(pendingId))
        }
        try {
            actor.call(Ops.send(chatId, text, images))
        } catch (error: Throwable) {
            pendingTimer?.cancel()
            pendingTimer = null
            stateMutex.withLock {
                _state.value = _state.value.copy(
                    pendingUser = null,
                    error = error.message ?: "Could not send the message",
                )
            }
            throw error
        }
    }

    suspend fun call(request: JsonObject) {
        actor.call(request)
    }

    suspend fun onEvent(event: SequencedEvent) {
        val value = event.value
        val eventChat = (value["chat"] as? JsonPrimitive)?.contentOrNull
        if (eventChat != null && eventChat != chatId) return

        when ((value["event"] as? JsonPrimitive)?.contentOrNull) {
            "turn-started" -> apply(
                TranscriptInput.TurnStarted(
                    label = (value["label"] as? JsonPrimitive)?.contentOrNull,
                    nowMillis = nowMillis(),
                ),
                event.sequence,
            )
            "text" -> (value["text"] as? JsonPrimitive)?.contentOrNull?.let {
                apply(TranscriptInput.Text(it), event.sequence)
            }
            "tool" -> (value["text"] as? JsonPrimitive)?.contentOrNull?.let {
                apply(TranscriptInput.Tool(it), event.sequence)
            }
            "turn-finished" -> apply(TranscriptInput.TurnFinished, event.sequence)
            "changed" -> apply(TranscriptInput.Changed, event.sequence)
            "queued" -> {
                val queue = (value["queue"] as? JsonArray)
                    ?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
                    .orEmpty()
                apply(TranscriptInput.Queued(queue), event.sequence)
            }
        }
    }

    fun shutdown() {
        pendingTimer?.cancel()
        pendingTimer = null
    }

    private suspend fun apply(
        input: TranscriptInput,
        sequence: Long? = null,
    ) {
        val effects = stateMutex.withLock {
            val transition = TranscriptMachine.reduce(_state.value, input)
            _state.value = transition.state
            if (reloadInProgress && input !is TranscriptInput.Loaded) {
                inputsDuringReload += BufferedInput(sequence, input)
            }
            transition.effects
        }
        if (input is TranscriptInput.TurnStarted || input is TranscriptInput.Queued) {
            pendingTimer?.cancel()
            pendingTimer = null
        }
        if (effects.isNotEmpty()) {
            scope.launch { reload() }
        }
    }

    private companion object {
        const val PENDING_TIMEOUT_MILLIS = 10_000L
    }

    private data class BufferedInput(
        val sequence: Long?,
        val input: TranscriptInput,
    )
}
