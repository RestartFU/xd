package com.restartfu.xd.net

import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.protocol.HelloReply
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.PairReply
import com.restartfu.xd.protocol.RemoteProtocolException
import com.restartfu.xd.protocol.RemoteRefusedException
import com.restartfu.xd.protocol.WireJson
import com.restartfu.xd.protocol.decodeReply
import com.restartfu.xd.protocol.requireSuccess
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject

public sealed interface Link {
    public data object Idle : Link
    public data object Connecting : Link
    public data class Up(val deviceName: String) : Link
    public data class Down(
        val message: String,
        val nextAttemptInMs: Long,
    ) : Link

    public data class Fatal(
        val reason: FatalReason,
        val message: String,
    ) : Link
}

public enum class FatalReason {
    PIN_MISMATCH,
    UNKNOWN_DEVICE,
    PROTOCOL,
}

public sealed interface PairResult {
    public data class Success(val deviceName: String) : PairResult
    public data class Failure(val message: String) : PairResult
}

public class NotConnectedException(
    message: String = "Not connected to the daemon",
) : IllegalStateException(message)

/**
 * One call passed its deadline. The connection stays up: its reply slot is
 * abandoned so a late reply cannot be handed to a later caller.
 */
public class CallTimedOutException(
    message: String = "The daemon did not answer in time",
) : Exception(message)

private class DisconnectedException(
    message: String,
) : Exception(message)

internal data class SequencedReply(
    val sequence: Long,
    val value: JsonObject,
)

internal data class SequencedEvent(
    val sequence: Long,
    val value: JsonObject,
)

/**
 * Owns connection state and is the only coroutine touching protocol queues.
 *
 * Socket callbacks append messages to a bounded channel. Parsing, greeting,
 * FIFO matching, and reconnect decisions happen in [run].
 */
internal class ConnectionActor(
    private val socketFactory: PlatformSocketFactory,
    private val credentialStore: CredentialStore,
    private val scope: CoroutineScope,
) {
    private val mailbox = Channel<Message>(MAILBOX_CAPACITY)
    private val backoff = Backoff()
    private val assembler = LineAssembler()
    private val _link = MutableStateFlow<Link>(Link.Idle)
    private val _hasCredentials = MutableStateFlow(false)
    private val _credentialsReady = MutableStateFlow(false)
    private val _events = MutableSharedFlow<SequencedEvent>(extraBufferCapacity = 1024)
    private var credentials: StoredCredentials? = null
    private var socket: PlatformSocket? = null
    private var leafCertificateDer: ByteArray? = null
    private var generation: Long = 0
    private var inboundSequence: Long = 0
    private var wanted: Boolean = true
    private var retryJob: Job? = null
    private var pairing: PairAttempt? = null
    private val callTimeouts = mutableMapOf<Long, Job>()
    private val calls = CallQueue(
        write = { bytes ->
            socket?.send(bytes) ?: throw NotConnectedException()
        },
    )

    val link: StateFlow<Link> = _link.asStateFlow()
    val hasCredentials: StateFlow<Boolean> = _hasCredentials.asStateFlow()
    val credentialsReady: StateFlow<Boolean> = _credentialsReady.asStateFlow()
    val events: SharedFlow<SequencedEvent> = _events.asSharedFlow()

    init {
        scope.launch { run() }
    }

    fun poke() {
        sendToMailbox(Message.Poke)
    }

    fun goBackground() {
        sendToMailbox(Message.Background)
    }

    suspend fun pair(
        host: String,
        port: Int,
        code: String,
        deviceName: String,
    ): PairResult {
        require(host.isNotBlank()) { "Host must not be blank" }
        require(port in 1..65535) { "Port must be between 1 and 65535" }
        require(code.isNotBlank()) { "Pairing code must not be blank" }
        require(deviceName.isNotBlank()) { "Device name must not be blank" }

        val result = CompletableDeferred<PairResult>()
        mailbox.send(
            Message.Pair(
                host = host,
                port = port,
                code = code,
                deviceName = deviceName,
                result = result,
            ),
        )
        return result.await()
    }

    suspend fun forget() {
        val done = CompletableDeferred<Unit>()
        mailbox.send(Message.Forget(done))
        done.await()
    }

    suspend fun call(request: JsonObject): JsonObject {
        return callSequenced(request).value
    }

    suspend fun callSequenced(request: JsonObject): SequencedReply {
        val response = CompletableDeferred<SequencedReply>()
        mailbox.send(Message.Call(request, response))
        val reply = response.await()
        return try {
            reply.also { it.value.requireSuccess() }
        } catch (error: RemoteProtocolException) {
            reportProtocolFailure(error)
            throw error
        }
    }

    suspend fun <T> decodeReply(
        value: JsonObject,
        decode: (JsonObject) -> T,
    ): T {
        return try {
            decode(value)
        } catch (error: RemoteProtocolException) {
            reportProtocolFailure(error)
            throw error
        }
    }

    private suspend fun reportProtocolFailure(error: RemoteProtocolException) {
        val done = CompletableDeferred<Unit>()
        mailbox.send(Message.ProtocolFailure(error, done))
        done.await()
    }

    private suspend fun run() {
        credentials = credentialStore.load()
        _hasCredentials.value = credentials != null
        _credentialsReady.value = true
        mailbox.trySend(Message.Poke)
        for (message in mailbox) {
            when (message) {
                Message.Poke -> handlePoke()
                Message.Background -> handleBackground()
                is Message.Pair -> handlePair(message)
                is Message.Forget -> handleForget(message)
                is Message.Call -> handleCall(message)
                is Message.SocketConnected -> handleConnected(message)
                is Message.SocketBytes -> handleBytes(message)
                is Message.SocketClosed -> handleClosed(message)
                is Message.GreetingFinished -> handleGreeting(message)
                is Message.CallTimedOut -> handleCallTimedOut(message)
                is Message.ProtocolFailure -> handleProtocolFailure(message)
                is Message.Retry -> handleRetry()
            }
        }
    }

    private fun handlePoke() {
        wanted = true
        retryJob?.cancel()
        retryJob = null
        backoff.reset()

        if (_link.value is Link.Fatal) return
        if (socket == null && (pairing != null || credentials != null)) connect()
    }

    private fun handleBackground() {
        val fatal = _link.value as? Link.Fatal
        wanted = false
        retryJob?.cancel()
        retryJob = null
        pairing?.result?.complete(
            PairResult.Failure("Pairing cancelled when the app moved to background"),
        )
        pairing = null
        closeCurrent(DisconnectedException("App moved to background"))
        _link.value = fatal ?: Link.Idle
    }

    private fun handlePair(message: Message.Pair) {
        pairing?.result?.complete(PairResult.Failure("Superseded by another pairing attempt"))
        retryJob?.cancel()
        retryJob = null
        backoff.reset()
        wanted = true
        closeCurrent(DisconnectedException("Starting pairing"))
        pairing = PairAttempt(
            host = message.host,
            port = message.port,
            code = message.code,
            deviceName = message.deviceName,
            result = message.result,
        )
        connect()
    }

    private suspend fun handleForget(message: Message.Forget) {
        retryJob?.cancel()
        retryJob = null
        pairing?.result?.complete(PairResult.Failure("Pairing cancelled"))
        pairing = null
        try {
            credentialStore.clear()
        } catch (error: CancellationException) {
            throw error
        } catch (error: Throwable) {
            message.done.completeExceptionally(error)
            return
        }
        credentials = null
        _hasCredentials.value = false
        wanted = false
        closeCurrent(DisconnectedException("Remote forgotten"))
        _link.value = Link.Idle
        message.done.complete(Unit)
    }

    private fun handleCall(message: Message.Call) {
        if (_link.value !is Link.Up || socket == null) {
            message.response.completeExceptionally(NotConnectedException())
            return
        }

        try {
            val call = calls.enqueue(message.request, message.response)
            val deadline = timeoutMillisFor(message.request)
            val inboundMark = inboundSequence
            callTimeouts[call.id] = scope.launch {
                delay(deadline)
                mailbox.send(Message.CallTimedOut(call.id, inboundMark))
            }
        } catch (error: Throwable) {
            handleClosed(
                Message.SocketClosed(
                    generation,
                    SocketFailure(SocketFailureKind.IO, error.message ?: "Write failed"),
                ),
            )
        }
    }

    /**
     * Retires one timed-out call, keeping its reply slot reserved so a late
     * reply is discarded rather than handed to a later caller.
     *
     * Whether the connection survives depends on what else arrived. If not a
     * single frame was read since this call was written, the stream is dead
     * and reconnecting is the only way forward. If other replies or events did
     * arrive, the connection is demonstrably alive and only this one call was
     * slow -- killing it would take every other chat down with it.
     */
    private fun handleCallTimedOut(message: Message.CallTimedOut) {
        callTimeouts.remove(message.id)?.cancel()
        val response = calls.abandon(message.id) ?: return
        response.completeExceptionally(
            CallTimedOutException("The daemon did not answer in time"),
        )

        val silentStream = inboundSequence == message.inboundMark
        if (silentStream || calls.abandonedOverflow) {
            handleClosed(
                Message.SocketClosed(
                    generation,
                    SocketFailure(
                        SocketFailureKind.IO,
                        if (silentStream) {
                            "Daemon did not answer the request in time"
                        } else {
                            "Too many daemon requests timed out"
                        },
                    ),
                ),
            )
        }
    }

    private fun handleProtocolFailure(message: Message.ProtocolFailure) {
        protocolFatal("Daemon sent a reply with an invalid shape", message.error)
        message.done.complete(Unit)
    }

    private fun handleConnected(message: Message.SocketConnected) {
        if (message.generation != generation || socket == null) return
        leafCertificateDer = message.certificateDer.copyOf()
        val attempt = pairing
        val reply = CompletableDeferred<SequencedReply>()
        val request = if (attempt != null) {
            Ops.pair(attempt.code, attempt.deviceName)
        } else {
            val saved = credentials
                ?: return protocolFatal("Connected without credentials or pairing")
            Ops.hello(saved.token)
        }

        try {
            calls.enqueue(request, reply)
        } catch (error: Throwable) {
            handleClosed(
                Message.SocketClosed(
                    generation,
                    SocketFailure(SocketFailureKind.IO, error.message ?: "Greeting write failed"),
                ),
            )
            return
        }

        val greetingGeneration = generation
        scope.launch {
            val result = runCatching {
                withTimeout(GREETING_TIMEOUT_MILLIS) { reply.await() }
            }
            mailbox.send(Message.GreetingFinished(greetingGeneration, result))
        }
    }

    private suspend fun handleBytes(message: Message.SocketBytes) {
        if (message.generation != generation || socket == null) return
        try {
            for (line in assembler.append(message.bytes)) {
                val sequence = ++inboundSequence
                val objectValue = WireJson.parseToJsonElement(line).jsonObject
                if ("event" in objectValue) {
                    _events.emit(SequencedEvent(sequence, objectValue))
                } else {
                    calls.acceptReply(SequencedReply(sequence, objectValue))
                        ?.let { answered -> callTimeouts.remove(answered)?.cancel() }
                }
            }
        } catch (error: Throwable) {
            protocolFatal("Daemon sent invalid protocol data", error)
        }
    }

    private suspend fun handleGreeting(message: Message.GreetingFinished) {
        if (message.generation != generation || socket == null) return
        val raw = message.result.getOrElse {
            return handleClosed(
                Message.SocketClosed(
                    generation,
                    SocketFailure(SocketFailureKind.IO, it.message ?: "Greeting failed"),
                ),
            )
        }

        val value = raw.value
        try {
            value.requireSuccess()
            val attempt = pairing
            if (attempt != null) {
                val reply = value.decodeReply<PairReply>()
                require(reply.token.isNotBlank()) { "Pair reply has an empty token" }
                val certificate = leafCertificateDer
                    ?: throw RemoteProtocolException("Pairing returned no certificate")
                val saved = StoredCredentials(
                    host = attempt.host,
                    port = attempt.port,
                    token = reply.token,
                    certificateDer = certificate.copyOf(),
                )
                credentialStore.save(saved)
                credentials = saved
                pairing = null
                _hasCredentials.value = true
                backoff.reset()
                _link.value = Link.Up(attempt.deviceName)
                attempt.result.complete(PairResult.Success(attempt.deviceName))
            } else {
                val reply = value.decodeReply<HelloReply>()
                if (reply.version != PROTOCOL_VERSION) {
                    return protocolFatal(
                        "Unsupported daemon protocol version ${reply.version}",
                    )
                }
                backoff.reset()
                _link.value = Link.Up(reply.device)
            }
        } catch (error: RemoteRefusedException) {
            val attempt = pairing
            pairing = null
            wanted = false
            closeCurrent(error)
            _link.value = if (attempt == null) {
                Link.Fatal(FatalReason.UNKNOWN_DEVICE, error.message.orEmpty())
            } else {
                Link.Idle
            }
            attempt?.result?.complete(PairResult.Failure(error.message.orEmpty()))
        } catch (error: CancellationException) {
            throw error
        } catch (error: Throwable) {
            val attempt = pairing
            pairing = null
            attempt?.result?.complete(
                PairResult.Failure(error.message ?: "Invalid pairing reply"),
            )
            protocolFatal("Invalid greeting reply", error)
        }
    }

    private fun handleClosed(message: Message.SocketClosed) {
        if (message.generation != generation) return
        val reason = message.failure
        val text = reason?.message ?: "Daemon closed the connection"
        val old = socket
        socket = null
        leafCertificateDer = null
        assembler.reset()
        cancelCallTimeouts()
        calls.failAll(DisconnectedException(text))
        old?.close()

        val attempt = pairing
        if (attempt != null) {
            pairing = null
            wanted = false
            _link.value = Link.Idle
            attempt.result.complete(PairResult.Failure(text))
            return
        }

        if (reason?.kind == SocketFailureKind.PIN_MISMATCH) {
            wanted = false
            _link.value = Link.Fatal(FatalReason.PIN_MISMATCH, text)
            return
        }

        if (!wanted || credentials == null || _link.value is Link.Fatal) {
            _link.value = if (_link.value is Link.Fatal) _link.value else Link.Idle
            return
        }

        scheduleReconnect(text)
    }

    private fun handleRetry() {
        retryJob = null
        if (wanted && credentials != null && socket == null && _link.value !is Link.Fatal) {
            connect()
        }
    }

    private fun connect() {
        check(socket == null)
        val attempt = pairing
        val saved = credentials
        val host = attempt?.host ?: saved?.host ?: return
        val port = attempt?.port ?: saved?.port ?: return
        val pin = if (attempt != null) null else saved?.certificateDer
        check(pin != null || attempt != null) {
            "Unpinned TLS is legal only during pairing"
        }

        generation += 1
        val thisGeneration = generation
        assembler.reset()
        leafCertificateDer = null
        _link.value = Link.Connecting
        val created = try {
            socketFactory.create()
        } catch (error: Throwable) {
            handleClosed(
                Message.SocketClosed(
                    thisGeneration,
                    SocketFailure(
                        SocketFailureKind.IO,
                        error.message ?: "Could not create a socket",
                    ),
                ),
            )
            return
        }
        socket = created
        val listener = object : PlatformSocketListener {
            override fun onConnected(leafCertificateDer: ByteArray) {
                sendToMailbox(
                    Message.SocketConnected(thisGeneration, leafCertificateDer.copyOf()),
                )
            }

            override fun onBytes(chunk: ByteArray) {
                if (
                    mailbox.trySend(
                        Message.SocketBytes(thisGeneration, chunk.copyOf()),
                    ).isFailure
                ) {
                    created.close()
                    sendToMailbox(
                        Message.SocketClosed(
                            thisGeneration,
                            SocketFailure(
                                SocketFailureKind.IO,
                                "Inbound protocol buffer overflow",
                            ),
                        ),
                    )
                }
            }

            override fun onClosed(reason: SocketFailure?) {
                sendToMailbox(Message.SocketClosed(thisGeneration, reason))
            }
        }

        try {
            created.connect(host, port, pin?.copyOf(), listener)
        } catch (error: Throwable) {
            handleClosed(
                Message.SocketClosed(
                    thisGeneration,
                    SocketFailure(SocketFailureKind.IO, error.message ?: "Connect failed"),
                ),
            )
        }
    }

    private fun scheduleReconnect(message: String) {
        retryJob?.cancel()
        val wait = backoff.nextDelayMillis()
        _link.value = Link.Down(message, wait)
        retryJob = scope.launch {
            delay(wait)
            mailbox.send(Message.Retry)
        }
    }

    private fun protocolFatal(
        message: String,
        cause: Throwable? = null,
    ) {
        wanted = false
        val error = RemoteProtocolException(message, cause)
        pairing?.result?.complete(PairResult.Failure(message))
        pairing = null
        closeCurrent(error)
        _link.value = Link.Fatal(FatalReason.PROTOCOL, message)
    }

    private fun closeCurrent(error: Throwable) {
        generation += 1
        val old = socket
        socket = null
        leafCertificateDer = null
        assembler.reset()
        cancelCallTimeouts()
        calls.failAll(error)
        old?.close()
    }

    private fun cancelCallTimeouts() {
        callTimeouts.values.forEach(Job::cancel)
        callTimeouts.clear()
    }

    private fun sendToMailbox(message: Message) {
        if (mailbox.trySend(message).isFailure) {
            scope.launch { mailbox.send(message) }
        }
    }

    private data class PairAttempt(
        val host: String,
        val port: Int,
        val code: String,
        val deviceName: String,
        val result: CompletableDeferred<PairResult>,
    )

    private sealed interface Message {
        data object Poke : Message
        data object Background : Message
        data object Retry : Message

        data class Pair(
            val host: String,
            val port: Int,
            val code: String,
            val deviceName: String,
            val result: CompletableDeferred<PairResult>,
        ) : Message

        data class Forget(val done: CompletableDeferred<Unit>) : Message

        data class Call(
            val request: JsonObject,
            val response: CompletableDeferred<SequencedReply>,
        ) : Message

        data class CallTimedOut(
            val id: Long,
            val inboundMark: Long,
        ) : Message

        data class ProtocolFailure(
            val error: RemoteProtocolException,
            val done: CompletableDeferred<Unit>,
        ) : Message

        data class SocketConnected(
            val generation: Long,
            val certificateDer: ByteArray,
        ) : Message

        data class SocketBytes(
            val generation: Long,
            val bytes: ByteArray,
        ) : Message

        data class SocketClosed(
            val generation: Long,
            val failure: SocketFailure?,
        ) : Message

        data class GreetingFinished(
            val generation: Long,
            val result: Result<SequencedReply>,
        ) : Message
    }

    /**
     * Mirrors `Daemon::Client`: Git actions may legitimately run for minutes,
     * everything else is expected to answer promptly.
     */
    private fun timeoutMillisFor(request: JsonObject): Long {
        val op = (request["op"] as? JsonPrimitive)?.contentOrNull
        return if (op in LONG_OPERATIONS) {
            LONG_CALL_TIMEOUT_MILLIS
        } else {
            CALL_TIMEOUT_MILLIS
        }
    }

    private companion object {
        const val GREETING_TIMEOUT_MILLIS = 15_000L
        const val CALL_TIMEOUT_MILLIS = 30_000L
        const val LONG_CALL_TIMEOUT_MILLIS = 5 * 60_000L
        const val MAILBOX_CAPACITY = 64
        const val PROTOCOL_VERSION = 1
        val LONG_OPERATIONS = setOf("git-action")
    }
}
