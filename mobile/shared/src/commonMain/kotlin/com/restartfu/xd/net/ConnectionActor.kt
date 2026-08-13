package com.restartfu.xd.net

import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.StoredCredentials
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.RemoteProtocolException
import com.restartfu.xd.protocol.WireJson
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
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject

public sealed interface Link {
    public data object Idle : Link
    public data object Connecting : Link
    public data class Up(val remoteName: String) : Link
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
    HOST_KEY_MISMATCH,
    AUTHENTICATION,
    PROTOCOL,
}

public sealed interface ConnectResult {
    public data class Success(val remoteName: String) : ConnectResult
    public data class HostKeyVerificationRequired(
        val hostKey: SshHostKey,
    ) : ConnectResult
    public data class Failure(val message: String) : ConnectResult
}

public class NotConnectedException(
    message: String = "Not connected to the host",
) : IllegalStateException(message)

/**
 * One call passed its deadline. The connection stays up: its reply slot is
 * abandoned so a late reply cannot be handed to a later caller.
 */
public class CallTimedOutException(
    message: String = "The host did not answer in time",
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
 * Socket callbacks append messages to a bounded channel. Parsing, FIFO
 * matching, and reconnect decisions happen in [run].
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
    private var generation: Long = 0
    private var inboundSequence: Long = 0
    private var wanted: Boolean = true
    private var retryJob: Job? = null
    private var connectionAttempt: ConnectionAttempt? = null
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

    suspend fun connect(connection: SshConnection): ConnectResult {
        validateConnection(connection)
        val result = CompletableDeferred<ConnectResult>()
        mailbox.send(Message.Connect(connection, result))
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
                is Message.Connect -> handleConnect(message)
                is Message.Forget -> handleForget(message)
                is Message.Call -> handleCall(message)
                is Message.SocketConnected -> handleConnected(message)
                is Message.SocketBytes -> handleBytes(message)
                is Message.SocketClosed -> handleClosed(message)
                is Message.ReadinessSucceeded -> handleReadinessSucceeded(message)
                is Message.ReadinessFailed -> handleReadinessFailed(message)
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
        if (socket == null && (connectionAttempt != null || credentials != null)) connectSocket()
    }

    private fun handleBackground() {
        val fatal = _link.value as? Link.Fatal
        wanted = false
        retryJob?.cancel()
        retryJob = null
        connectionAttempt?.result?.complete(
            ConnectResult.Failure("Connection cancelled when the app moved to background"),
        )
        connectionAttempt = null
        closeCurrent(DisconnectedException("App moved to background"))
        _link.value = fatal ?: Link.Idle
    }

    private fun handleConnect(message: Message.Connect) {
        connectionAttempt?.result?.complete(
            ConnectResult.Failure("Superseded by another connection attempt"),
        )
        retryJob?.cancel()
        retryJob = null
        backoff.reset()
        wanted = true
        closeCurrent(DisconnectedException("Starting connection"))
        connectionAttempt = ConnectionAttempt(message.connection, message.result)
        connectSocket()
    }

    private suspend fun handleForget(message: Message.Forget) {
        retryJob?.cancel()
        retryJob = null
        connectionAttempt?.result?.complete(ConnectResult.Failure("Connection cancelled"))
        connectionAttempt = null
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
            CallTimedOutException("The host did not answer in time"),
        )

        val silentStream = inboundSequence == message.inboundMark
        if (silentStream || calls.abandonedOverflow) {
            handleClosed(
                Message.SocketClosed(
                    generation,
                    SocketFailure(
                        SocketFailureKind.IO,
                        if (silentStream) {
                            "Host did not answer the request in time"
                        } else {
                            "Too many host requests timed out"
                        },
                    ),
                ),
            )
        }
    }

    private fun handleProtocolFailure(message: Message.ProtocolFailure) {
        protocolFatal("Host sent a reply with an invalid shape", message.error)
        message.done.complete(Unit)
    }

    private suspend fun handleConnected(message: Message.SocketConnected) {
        if (message.generation != generation || socket == null) return
        val connection = connectionAttempt?.connection ?: credentials?.connection
            ?: return protocolFatal("Connected without SSH credentials")
        if (connection.hostKey == null) {
            return handleClosed(
                Message.SocketClosed(
                    generation,
                    SocketFailure(SocketFailureKind.IO, "SSH connected without a verified host key"),
                ),
            )
        }
        val response = CompletableDeferred<SequencedReply>()
        val call = try {
            calls.enqueue(Ops.tree(), response)
        } catch (error: Throwable) {
            handleClosed(
                Message.SocketClosed(
                    generation,
                    SocketFailure(SocketFailureKind.IO, error.message ?: "Write failed"),
                ),
            )
            return
        }
        val inboundMark = inboundSequence
        callTimeouts[call.id] = scope.launch {
            delay(CALL_TIMEOUT_MILLIS)
            mailbox.send(Message.CallTimedOut(call.id, inboundMark))
        }
        val probeGeneration = generation
        scope.launch {
            try {
                response.await().value.requireSuccess()
                mailbox.send(Message.ReadinessSucceeded(probeGeneration))
            } catch (error: Throwable) {
                if (error is CancellationException) throw error
                mailbox.send(Message.ReadinessFailed(probeGeneration, error))
            }
        }
    }

    private suspend fun handleReadinessSucceeded(message: Message.ReadinessSucceeded) {
        if (message.generation != generation || socket == null) return
        val attempt = connectionAttempt
        val connection = attempt?.connection ?: credentials?.connection
            ?: return protocolFatal("Ready without SSH credentials")
        if (attempt != null) {
            val saved = StoredCredentials(connection)
            try {
                credentialStore.save(saved)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                connectionAttempt = null
                wanted = false
                closeCurrent(error)
                _link.value = Link.Idle
                attempt.result.complete(
                    ConnectResult.Failure(error.message ?: "Could not save SSH credentials"),
                )
                return
            }
            credentials = saved
            connectionAttempt = null
            _hasCredentials.value = true
            attempt.result.complete(ConnectResult.Success(remoteName(connection)))
        }
        backoff.reset()
        _link.value = Link.Up(remoteName(connection))
    }

    private fun handleReadinessFailed(message: Message.ReadinessFailed) {
        if (message.generation != generation || socket == null) return
        protocolFatal("Host did not complete xd-host protocol setup", message.error)
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
            protocolFatal("Host sent invalid protocol data", error)
        }
    }

    private fun handleClosed(message: Message.SocketClosed) {
        if (message.generation != generation) return
        val reason = message.failure
        val text = reason?.message ?: "Host closed the connection"
        val old = socket
        socket = null
        assembler.reset()
        cancelCallTimeouts()
        calls.failAll(DisconnectedException(text))
        old?.close()

        val attempt = connectionAttempt
        if (attempt != null) {
            connectionAttempt = null
            wanted = false
            _link.value = Link.Idle
            val result = if (
                reason?.kind == SocketFailureKind.HOST_KEY_UNKNOWN && reason.hostKey != null
            ) {
                ConnectResult.HostKeyVerificationRequired(reason.hostKey)
            } else {
                ConnectResult.Failure(text)
            }
            attempt.result.complete(result)
            return
        }

        if (reason?.kind == SocketFailureKind.HOST_KEY_MISMATCH) {
            wanted = false
            _link.value = Link.Fatal(FatalReason.HOST_KEY_MISMATCH, text)
            return
        }
        if (reason?.kind == SocketFailureKind.AUTHENTICATION) {
            wanted = false
            _link.value = Link.Fatal(FatalReason.AUTHENTICATION, text)
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
            connectSocket()
        }
    }

    private fun connectSocket() {
        check(socket == null)
        val connection = connectionAttempt?.connection ?: credentials?.connection ?: return

        generation += 1
        val thisGeneration = generation
        assembler.reset()
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
            override fun onConnected() {
                sendToMailbox(Message.SocketConnected(thisGeneration))
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
            created.connect(connection, listener)
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
        connectionAttempt?.result?.complete(ConnectResult.Failure(message))
        connectionAttempt = null
        closeCurrent(error)
        _link.value = Link.Fatal(FatalReason.PROTOCOL, message)
    }

    private fun closeCurrent(error: Throwable) {
        generation += 1
        val old = socket
        socket = null
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

    private data class ConnectionAttempt(
        val connection: SshConnection,
        val result: CompletableDeferred<ConnectResult>,
    )

    private sealed interface Message {
        data object Poke : Message
        data object Background : Message
        data object Retry : Message

        data class Connect(
            val connection: SshConnection,
            val result: CompletableDeferred<ConnectResult>,
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
        ) : Message

        data class SocketBytes(
            val generation: Long,
            val bytes: ByteArray,
        ) : Message

        data class SocketClosed(
            val generation: Long,
            val failure: SocketFailure?,
        ) : Message

        data class ReadinessSucceeded(
            val generation: Long,
        ) : Message

        data class ReadinessFailed(
            val generation: Long,
            val error: Throwable,
        ) : Message

    }

    /**
     * Mirrors `Host::Client`: Git actions may legitimately run for minutes,
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
        const val CALL_TIMEOUT_MILLIS = 30_000L
        const val LONG_CALL_TIMEOUT_MILLIS = 5 * 60_000L
        const val MAILBOX_CAPACITY = 64
        val LONG_OPERATIONS = setOf("git-action")
    }
}

private fun validateConnection(connection: SshConnection) {
    require(connection.host.isNotBlank()) { "Host must not be blank" }
    require(connection.port in 1..65535) { "Port must be between 1 and 65535" }
    require(connection.username.isNotBlank()) { "Username must not be blank" }
    when (val authentication = connection.authentication) {
        is com.restartfu.xd.credentials.SshAuthentication.Password ->
            require(authentication.value.isNotEmpty()) { "Password must not be empty" }
        is com.restartfu.xd.credentials.SshAuthentication.PrivateKey ->
            require(authentication.bytes.isNotEmpty()) { "Private key must not be empty" }
    }
    connection.hostKey?.let {
        require(it.algorithm.isNotBlank()) { "Host-key algorithm must not be blank" }
        require(it.encoded.isNotEmpty()) { "Host key must not be empty" }
        require(it.fingerprint.isNotBlank()) { "Host-key fingerprint must not be blank" }
    }
}

private fun remoteName(connection: SshConnection): String =
    "${connection.username}@${connection.host}"
