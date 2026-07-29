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
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
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

    public suspend fun loadOlder(): Unit = core.loadOlder()

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
    private val sendMutex = Mutex()
    private val _state = MutableStateFlow(ChatState(chatId = chatId))
    private var pendingSequence = 0L
    private var pendingTimer: Job? = null
    private var messageLimit = MESSAGE_PAGE_SIZE
    private var reloadInProgress = false
    private var snapshotSequence = 0L
    private var invalidated = false
    private val inputsDuringReload = mutableListOf<BufferedInput>()
    private val reloadRequests = Channel<Unit>(Channel.CONFLATED)
    private val reloadWorker = scope.launch {
        for (ignored in reloadRequests) reload()
    }

    val state: StateFlow<ChatState> = _state.asStateFlow()

    suspend fun reload() {
        reloadMutex.withLock {
            val shouldReload = stateMutex.withLock {
                if (invalidated) return@withLock false
                reloadInProgress = true
                inputsDuringReload.clear()
                _state.value = _state.value.copy(loading = true, error = null)
                true
            }
            if (!shouldReload) return@withLock
            try {
                val chatReply = actor.callSequenced(Ops.chat(chatId))
                val chat = actor.decodeReply(chatReply.value) {
                    it.decodeReply<ChatReply>()
                }
                val messagesValue = actor.call(Ops.messages(chatId, messageLimit))
                val messages = actor.decodeReply(messagesValue) {
                    it.decodeReply<MessagesReply>()
                }
                val replayEffects = stateMutex.withLock {
                    if (invalidated) return@withLock emptyList()
                    snapshotSequence = maxOf(snapshotSequence, chatReply.sequence)
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
                if (replayEffects.isNotEmpty()) requestReload()
            } catch (error: CancellationException) {
                clearReloadAfterCancellation()
                throw error
            } catch (error: Throwable) {
                stateMutex.withLock {
                    if (invalidated) return@withLock
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

    suspend fun loadOlder() {
        reloadMutex.withLock {
            val shouldLoad = stateMutex.withLock {
                if (
                    invalidated ||
                    !_state.value.hasOlderMessages ||
                    _state.value.loadingOlder
                ) {
                    false
                } else {
                    _state.value = _state.value.copy(
                        loadingOlder = true,
                        error = null,
                    )
                    true
                }
            }
            if (!shouldLoad) return@withLock

            val nextLimit = minOf(
                messageLimit.toLong() + MESSAGE_PAGE_SIZE,
                Int.MAX_VALUE.toLong(),
            ).toInt()
            try {
                val messagesValue = actor.call(Ops.messages(chatId, nextLimit))
                val messages = actor.decodeReply(messagesValue) {
                    it.decodeReply<MessagesReply>()
                }
                stateMutex.withLock {
                    if (invalidated) return@withLock
                    _state.value = TranscriptMachine.reduce(
                        _state.value,
                        TranscriptInput.MessagesLoaded(messages),
                    ).state
                    messageLimit = nextLimit
                }
            } catch (error: CancellationException) {
                withContext(NonCancellable) {
                    stateMutex.withLock {
                        if (!invalidated) {
                            _state.value = _state.value.copy(loadingOlder = false)
                        }
                    }
                }
                throw error
            } catch (error: Throwable) {
                stateMutex.withLock {
                    if (invalidated) return@withLock
                    _state.value = _state.value.copy(
                        loadingOlder = false,
                        error = error.message ?: "Could not load older messages",
                    )
                }
            }
        }
    }

    suspend fun send(
        text: String,
        images: List<PngAttachment>,
    ) = sendMutex.withLock {
        ensureActive()
        require(text.isNotEmpty() || images.isNotEmpty())
        lateinit var timer: Job
        var oldTimer: Job? = null
        val pendingId = stateMutex.withLock {
            val id = "pending-${++pendingSequence}"
            val optimistic = TranscriptInput.OptimisticSend(id, text, nowMillis())
            val transition = TranscriptMachine.reduce(
                _state.value,
                optimistic,
            )
            _state.value = transition.state
            if (reloadInProgress) {
                inputsDuringReload += BufferedInput(
                    sequence = null,
                    input = optimistic,
                )
            }
            timer = scope.launch(start = CoroutineStart.LAZY) {
                delay(PENDING_TIMEOUT_MILLIS)
                apply(TranscriptInput.PendingTimedOut(id))
            }
            oldTimer = pendingTimer
            pendingTimer = timer
            id
        }
        oldTimer?.cancel()
        timer.start()
        try {
            actor.call(Ops.send(chatId, text, images))
        } catch (error: CancellationException) {
            throw error
        } catch (error: Throwable) {
            apply(
                TranscriptInput.SendFailed(
                    pendingId,
                    error.message ?: "Could not send the message",
                ),
            )
            throw error
        }
        Unit
    }

    suspend fun call(request: JsonObject) {
        ensureActive()
        try {
            actor.call(request)
        } catch (error: CancellationException) {
            throw error
        } catch (error: Throwable) {
            apply(
                TranscriptInput.RequestFailed(
                    error.message ?: "The daemon refused the request",
                ),
            )
            throw error
        }
    }

    suspend fun onEvent(event: SequencedEvent) {
        if (stateMutex.withLock { invalidated }) return
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
            "commands" -> {
                val commands = (value["commands"] as? JsonArray)
                    ?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
                    .orEmpty()
                apply(TranscriptInput.Commands(commands), event.sequence)
            }
            "queued" -> {
                val queue = (value["queue"] as? JsonArray)
                    ?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
                    .orEmpty()
                apply(TranscriptInput.Queued(queue), event.sequence)
            }
        }
    }

    fun shutdown() {
        reloadRequests.close()
        reloadWorker.cancel()
        scope.launch {
            val timer = stateMutex.withLock {
                pendingTimer.also { pendingTimer = null }
            }
            timer?.cancel()
        }
    }

    suspend fun invalidate() {
        reloadRequests.close()
        reloadWorker.cancel()
        val timer = stateMutex.withLock {
            invalidated = true
            reloadInProgress = false
            inputsDuringReload.clear()
            messageLimit = MESSAGE_PAGE_SIZE
            snapshotSequence = 0L
            pendingSequence = 0L
            _state.value = ChatState(chatId = chatId)
            pendingTimer.also { pendingTimer = null }
        }
        timer?.cancel()
    }

    fun requestReload() {
        reloadRequests.trySend(Unit)
    }

    private suspend fun apply(
        input: TranscriptInput,
        sequence: Long? = null,
    ) {
        var timerToCancel: Job? = null
        val effects = stateMutex.withLock {
            if (
                invalidated ||
                (sequence != null && sequence <= snapshotSequence)
            ) {
                return@withLock emptyList()
            }
            val transition = TranscriptMachine.reduce(_state.value, input)
            _state.value = transition.state
            if (reloadInProgress && input !is TranscriptInput.Loaded) {
                inputsDuringReload += BufferedInput(sequence, input)
            }
            if (
                input is TranscriptInput.TurnStarted ||
                input is TranscriptInput.Queued ||
                input is TranscriptInput.SendFailed ||
                input is TranscriptInput.PendingTimedOut
            ) {
                timerToCancel = pendingTimer
                pendingTimer = null
            }
            transition.effects
        }
        timerToCancel?.cancel()
        if (effects.isNotEmpty()) {
            requestReload()
        }
    }

    private suspend fun ensureActive() {
        stateMutex.withLock {
            check(!invalidated) { "This chat belongs to a forgotten remote" }
        }
    }

    private suspend fun clearReloadAfterCancellation() {
        withContext(NonCancellable) {
            stateMutex.withLock {
                if (!invalidated) {
                    reloadInProgress = false
                    inputsDuringReload.clear()
                    _state.value = _state.value.copy(loading = false)
                }
            }
        }
    }

    private companion object {
        const val MESSAGE_PAGE_SIZE = 150
        const val PENDING_TIMEOUT_MILLIS = 10_000L
    }

    private data class BufferedInput(
        val sequence: Long?,
        val input: TranscriptInput,
    )
}
