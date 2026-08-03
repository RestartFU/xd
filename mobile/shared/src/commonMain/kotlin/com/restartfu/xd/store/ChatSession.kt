package com.restartfu.xd.store

import com.restartfu.xd.model.ChatState
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.net.SequencedEvent
import com.restartfu.xd.protocol.AgentCatalogReply
import com.restartfu.xd.protocol.BackendReply
import com.restartfu.xd.protocol.BrowseListReply
import com.restartfu.xd.protocol.BrowseReadReply
import com.restartfu.xd.protocol.ChatOption
import com.restartfu.xd.protocol.ChatReply
import com.restartfu.xd.protocol.DiffReply
import com.restartfu.xd.protocol.FileEntryReply
import com.restartfu.xd.protocol.ImageReply
import com.restartfu.xd.protocol.Limits
import kotlin.io.encoding.Base64
import kotlin.io.encoding.ExperimentalEncodingApi
import com.restartfu.xd.protocol.MessagesReply
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.PngAttachment
import com.restartfu.xd.protocol.TerminalListReply
import com.restartfu.xd.protocol.TerminalOpenReply
import com.restartfu.xd.protocol.TerminalReply
import com.restartfu.xd.protocol.VoiceModelReply
import com.restartfu.xd.protocol.decodeReply
import com.restartfu.xd.voice.SpeakTagParser
import com.restartfu.xd.voice.VoiceTransport
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull

public class ChatSession internal constructor(
    private val core: ChatSessionCore,
    private val release: () -> Unit,
) : AutoCloseable, VoiceTransport {
    public val state: StateFlow<ChatState> = core.state
    /** Complete assistant `<speak>` blocks from this live turn. */
    public val speech: Flow<String> = core.speech
    private var closed = false

    /** Drops any speech block that was in progress when speech was disabled. */
    public suspend fun resetSpeech(): Unit = core.resetSpeech()

    public suspend fun send(
        text: String,
        images: List<PngAttachment> = emptyList(),
    ): Unit = core.send(text, images)

    public suspend fun cancel(): Unit = core.call(Ops.cancel(core.chatId))

    public suspend fun enqueue(text: String): Unit = core.call(Ops.queue(core.chatId, text))

    public suspend fun setDraft(
        text: String,
        images: List<PngAttachment>? = null,
    ): Unit = core.call(Ops.setDraft(core.chatId, text, images))

    public suspend fun dropQueued(index: Int? = null): Unit =
        core.call(Ops.dropQueue(core.chatId, index))

    public suspend fun editQueued(
        index: Int,
        oldText: String,
        text: String,
    ): Unit = core.call(Ops.editQueue(core.chatId, index, oldText, text))

    /** Promotes a queued message and stops the running turn to take it up. */
    public suspend fun steerQueued(index: Int, text: String): Unit =
        core.call(Ops.steerQueue(core.chatId, index, text))

    public suspend fun loadOlder(): Unit = core.loadOlder()

    /** The whole patch for [read], one of `working-all` or `branch-all`. */
    public suspend fun diff(read: String, base: String? = null): String =
        core.read(Ops.diffRead(core.chatId, read, base)) {
            it.decodeReply<DiffReply>().output
        }

    /** The branch point `branch-all` should be read against. */
    public suspend fun diffBase(): String =
        core.read(Ops.diffRead(core.chatId, "base")) {
            it.decodeReply<DiffReply>().output.trim()
        }

    public suspend fun listDirectory(path: String?): List<FileEntryReply> =
        core.read(Ops.listDirectory(core.chatId, path)) {
            it.decodeReply<BrowseListReply>().entries
        }

    public suspend fun readFile(path: String): String =
        core.read(Ops.readFile(core.chatId, path)) {
            it.decodeReply<BrowseReadReply>().content
        }

    public suspend fun terminals(): List<TerminalReply> =
        core.read(Ops.terminalList(core.chatId)) {
            it.decodeReply<TerminalListReply>().terminals
        }

    public suspend fun openTerminal(
        columns: Int,
        rows: Int,
        reuse: Boolean,
    ): String = core.read(Ops.terminalOpen(core.chatId, columns, rows, reuse)) {
        it.decodeReply<TerminalOpenReply>().id
    }

    public suspend fun sendTerminalInput(terminalId: String, data: String): Unit =
        core.call(Ops.terminalInput(terminalId, data))

    public suspend fun resizeTerminal(
        terminalId: String,
        columns: Int,
        rows: Int,
    ): Unit = core.call(Ops.terminalResize(terminalId, columns, rows))

    public suspend fun killTerminal(terminalId: String): Unit =
        core.call(Ops.terminalKill(terminalId))

    public suspend fun setOption(option: ChatOption, value: String): Unit =
        core.call(Ops.setOption(core.chatId, option, value))

    /** The daemon reads `plan` and `new-worktree` as the strings, not JSON. */
    public suspend fun setBoolOption(option: ChatOption, value: Boolean): Unit =
        core.call(Ops.setBoolOption(core.chatId, option, value))

    /** The bytes of a stored image, already decoded from the wire. */
    @OptIn(ExperimentalEncodingApi::class)
    public suspend fun readImage(path: String): ByteArray =
        core.read(Ops.imageRead(path)) {
            Base64.Default.decode(it.decodeReply<ImageReply>().data)
        }

    /** The assistants and models this daemon can run. */
    public suspend fun catalog(): List<BackendReply> =
        core.read(Ops.agentCatalog()) {
            it.decodeReply<AgentCatalogReply>().backends
        }

    /** Selects assistant and model together, which is the validated path. */
    public suspend fun selectModel(backend: String, model: String): Unit =
        core.call(Ops.selectModel(core.chatId, backend, model))

    /**
     * Whether the machine running this chat has the speech model on disk.
     *
     * Transcription happens where the chat happens, so a remote chat asks the
     * remote machine -- and its answer can differ from another chat's.
     */
    override suspend fun voiceModelAvailable(): Boolean =
        core.read(Ops.voiceModel(core.chatId)) {
            it.decodeReply<VoiceModelReply>().available
        }

    /** Starts the download; progress arrives as `voice` events, not a reply. */
    override suspend fun downloadVoiceModel(token: String) {
        core.read(Ops.voiceModelDownload(core.chatId, token)) { }
    }

    /** Queues a recording for transcription; the text arrives as an event. */
    @OptIn(ExperimentalEncodingApi::class)
    override suspend fun transcribe(token: String, wav: ByteArray) {
        core.read(
            Ops.voiceTranscribe(core.chatId, token, Base64.Default.encode(wav)),
        ) { }
    }

    override suspend fun cancelVoice(token: String) {
        core.read(Ops.voiceCancel(token)) { }
    }

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
    private val _speech = MutableSharedFlow<String>(extraBufferCapacity = SPEECH_BUFFER_CAPACITY)
    private val speakParser = SpeakTagParser()
    private val speechMutex = Mutex()
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
    val speech: Flow<String> = _speech.asSharedFlow()

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

    /**
     * A read whose failure belongs to the pane that asked for it.
     *
     * Unlike [call], this does not push the error into the transcript: a
     * failed diff or directory listing is not a chat error and must not
     * surface as one.
     */
    suspend fun <T> read(
        request: JsonObject,
        decode: (JsonObject) -> T,
    ): T {
        ensureActive()
        val value = actor.call(request)
        return actor.decodeReply(value, decode)
    }

    suspend fun onEvent(event: SequencedEvent) {
        if (stateMutex.withLock { invalidated }) return
        val value = event.value
        val eventName = (value["event"] as? JsonPrimitive)?.contentOrNull
        val eventChat = (value["chat"] as? JsonPrimitive)?.contentOrNull
        if (eventName in CHAT_SCOPED_EVENTS && eventChat == null) return
        if (eventChat != null && eventChat != chatId) return

        val turnId = value.longOrNull("turn_id")
        val turnSequence = value.longOrNull("turn_sequence")

        // A tagged turn event is deduplicated by its turn watermark, not by
        // arrival order. Arrival order cannot distinguish "already folded into
        // the snapshot" from "generated after the snapshot was built but
        // written before its reply" -- gating on it would drop live deltas.
        // Untagged events keep the arrival gate as a compatibility path.
        val arrivalGate = if (turnId == null) event.sequence else null

        when (eventName) {
            "turn-started" -> {
                if (apply(
                        TranscriptInput.TurnStarted(
                            label = (value["label"] as? JsonPrimitive)?.contentOrNull,
                            nowMillis = nowMillis(),
                            turnId = turnId,
                            turnSequence = turnSequence,
                        ),
                        arrivalGate,
                    )
                ) {
                    speechMutex.withLock { speakParser.reset() }
                }
            }
            "text" -> (value["text"] as? JsonPrimitive)?.contentOrNull?.let { text ->
                if (apply(TranscriptInput.Text(text, turnId, turnSequence), arrivalGate)) {
                    val spoken = speechMutex.withLock { speakParser.feed(text) }
                    spoken.forEach { _speech.tryEmit(it) }
                }
            }
            "tool" -> (value["text"] as? JsonPrimitive)?.contentOrNull?.let { text ->
                if (apply(TranscriptInput.Tool(text, turnId, turnSequence), arrivalGate)) {
                    speechMutex.withLock { speakParser.reset() }
                }
            }
            "turn-finished" -> {
                if (apply(
                        TranscriptInput.TurnFinished(turnId, turnSequence),
                        arrivalGate,
                    )
                ) {
                    speechMutex.withLock { speakParser.reset() }
                }
            }
            "changed" -> apply(TranscriptInput.Changed, event.sequence)
            "shortcuts-changed" -> apply(TranscriptInput.Changed, event.sequence)
            "commands" -> {
                val commands = value.requiredStringArray("commands") ?: return
                apply(TranscriptInput.Commands(commands), event.sequence)
            }
            "queued" -> {
                val queue = value.requiredStringArray("queue") ?: return
                apply(TranscriptInput.Queued(queue), event.sequence)
            }
            "draft" -> {
                val text = (value["draft"] as? JsonPrimitive)?.contentOrNull ?: return
                val revision = value.longOrNull("draft_revision") ?: return
                val attachments = value["draft_attachments"]?.let {
                    decodeDraftAttachments(it as? JsonArray ?: return)
                }
                apply(
                    TranscriptInput.Draft(text, revision, attachments),
                    event.sequence,
                )
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
        resetSpeech()
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

    suspend fun resetSpeech() {
        speechMutex.withLock { speakParser.reset() }
    }

    fun requestReload() {
        reloadRequests.trySend(Unit)
    }

    private suspend fun apply(
        input: TranscriptInput,
        sequence: Long? = null,
    ): Boolean {
        var timerToCancel: Job? = null
        val result = stateMutex.withLock {
            if (
                invalidated ||
                (sequence != null && sequence <= snapshotSequence)
            ) {
                return@withLock false to emptyList<TranscriptEffect>()
            }
            val before = _state.value
            val transition = TranscriptMachine.reduce(before, input)
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
            (transition.state != before || transition.effects.isNotEmpty()) to transition.effects
        }
        timerToCancel?.cancel()
        if (result.second.isNotEmpty()) {
            requestReload()
        }
        return result.first
    }

    private suspend fun ensureActive() {
        stateMutex.withLock {
            check(!invalidated) { "This chat belongs to a forgotten remote" }
        }
    }

    private fun JsonObject.longOrNull(name: String): Long? =
        (this[name] as? JsonPrimitive)?.takeUnless { it.isString }?.longOrNull

    private fun JsonObject.requiredStringArray(name: String): List<String>? {
        val values = this[name] as? JsonArray ?: return null
        val decoded = mutableListOf<String>()
        for (value in values) {
            val primitive = value as? JsonPrimitive ?: return null
            if (!primitive.isString) return null
            decoded += primitive.content
        }
        return decoded
    }

    @OptIn(ExperimentalEncodingApi::class)
    private fun decodeDraftAttachments(values: JsonArray): List<PngAttachment>? {
        val decoded = mutableListOf<PngAttachment>()
        try {
            for (value in values) {
                val fields = value as? JsonObject ?: return null
                val mime = (fields["mime"] as? JsonPrimitive)?.contentOrNull
                val data = (fields["data"] as? JsonPrimitive)?.contentOrNull
                    ?: return null
                if (mime != Limits.PNG_MIME) return null
                decoded += PngAttachment(Base64.Default.decode(data))
            }
            Limits.validateImages(decoded)
        } catch (_: IllegalArgumentException) {
            return null
        }
        return decoded
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
        const val SPEECH_BUFFER_CAPACITY = 32
        val CHAT_SCOPED_EVENTS = setOf(
            "commands",
            "turn-started",
            "text",
            "tool",
            "turn-finished",
            "queued",
            "draft",
        )
    }

    private data class BufferedInput(
        val sequence: Long?,
        val input: TranscriptInput,
    )
}
