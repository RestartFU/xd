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

private class DisconnectedException(
    message: String,
) : Exception(message)

/**
 * Owns connection state and is the only coroutine touching protocol queues.
 *
 * Socket callbacks only append messages to an unlimited channel. Parsing,
 * greeting, FIFO matching, and reconnect decisions happen in [run].
 */
internal class ConnectionActor(
    private val socketFactory: PlatformSocketFactory,
    private val credentialStore: CredentialStore,
    private val scope: CoroutineScope,
) {
    private val mailbox = Channel<Message>(Channel.UNLIMITED)
    private val backoff = Backoff()
    private val assembler = LineAssembler()
    private val _link = MutableStateFlow<Link>(Link.Idle)
    private val _hasCredentials = MutableStateFlow(false)
    private val _events = MutableSharedFlow<JsonObject>(extraBufferCapacity = 1024)
    private var credentials: StoredCredentials? = credentialStore.load()
    private var socket: PlatformSocket? = null
    private var leafCertificateDer: ByteArray? = null
    private var generation: Long = 0
    private var wanted: Boolean = true
    private var retryJob: Job? = null
    private var pairing: PairAttempt? = null
    private val calls = CallQueue(
        write = { bytes ->
            socket?.send(bytes) ?: throw NotConnectedException()
        },
    )

    val link: StateFlow<Link> = _link.asStateFlow()
    val hasCredentials: StateFlow<Boolean> = _hasCredentials.asStateFlow()
    val events: SharedFlow<JsonObject> = _events.asSharedFlow()

    init {
        _hasCredentials.value = credentials != null
        scope.launch { run() }
        mailbox.trySend(Message.Poke)
    }

    fun poke() {
        mailbox.trySend(Message.Poke)
    }

    fun goBackground() {
        mailbox.trySend(Message.Background)
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
        val response = CompletableDeferred<JsonObject>()
        mailbox.send(Message.Call(request, response))
        return response.await().requireSuccess()
    }

    private suspend fun run() {
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
        wanted = false
        retryJob?.cancel()
        retryJob = null
        closeCurrent(DisconnectedException("App moved to background"))
        _link.value = Link.Idle
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

    private fun handleForget(message: Message.Forget) {
        retryJob?.cancel()
        retryJob = null
        pairing?.result?.complete(PairResult.Failure("Pairing cancelled"))
        pairing = null
        try {
            credentialStore.clear()
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
            calls.enqueue(message.request, message.response)
        } catch (error: Throwable) {
            mailbox.trySend(
                Message.SocketClosed(
                    generation,
                    SocketFailure(SocketFailureKind.IO, error.message ?: "Write failed"),
                ),
            )
        }
    }

    private fun handleConnected(message: Message.SocketConnected) {
        if (message.generation != generation || socket == null) return
        leafCertificateDer = message.certificateDer.copyOf()
        val attempt = pairing
        val reply = CompletableDeferred<JsonObject>()
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
            val result = runCatching { reply.await() }
            mailbox.send(Message.GreetingFinished(greetingGeneration, result))
        }
    }

    private suspend fun handleBytes(message: Message.SocketBytes) {
        if (message.generation != generation || socket == null) return
        try {
            for (line in assembler.append(message.bytes)) {
                val objectValue = WireJson.parseToJsonElement(line).jsonObject
                if ("event" in objectValue) {
                    _events.emit(objectValue)
                } else {
                    calls.acceptReply(objectValue)
                }
            }
        } catch (error: Throwable) {
            protocolFatal("Daemon sent invalid protocol data", error)
        }
    }

    private fun handleGreeting(message: Message.GreetingFinished) {
        if (message.generation != generation || socket == null) return
        val raw = message.result.getOrElse {
            return handleClosed(
                Message.SocketClosed(
                    generation,
                    SocketFailure(SocketFailureKind.IO, it.message ?: "Greeting failed"),
                ),
            )
        }

        try {
            raw.requireSuccess()
            val attempt = pairing
            if (attempt != null) {
                val reply = raw.decodeReply<PairReply>()
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
                val reply = raw.decodeReply<HelloReply>()
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
            _link.value = Link.Fatal(FatalReason.UNKNOWN_DEVICE, error.message.orEmpty())
            attempt?.result?.complete(PairResult.Failure(error.message.orEmpty()))
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
        socket = null
        leafCertificateDer = null
        assembler.reset()
        calls.failAll(DisconnectedException(text))

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
                mailbox.trySend(
                    Message.SocketConnected(thisGeneration, leafCertificateDer.copyOf()),
                )
            }

            override fun onBytes(chunk: ByteArray) {
                mailbox.trySend(Message.SocketBytes(thisGeneration, chunk.copyOf()))
            }

            override fun onClosed(reason: SocketFailure?) {
                mailbox.trySend(Message.SocketClosed(thisGeneration, reason))
            }
        }

        try {
            created.connect(host, port, pin?.copyOf(), listener)
        } catch (error: Throwable) {
            mailbox.trySend(
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
        calls.failAll(error)
        old?.close()
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
            val response: CompletableDeferred<JsonObject>,
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
            val result: Result<JsonObject>,
        ) : Message
    }

    private companion object {
        const val PROTOCOL_VERSION = 1
    }
}
