package com.restartfu.xd.voice

import com.restartfu.xd.time.currentEpochMillis
import kotlin.random.Random
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** Where a voice job has got to. */
public sealed interface VoiceState {
    public data object Idle : VoiceState

    /** Asking the daemon whether it has the speech model. */
    public data object Checking : VoiceState

    /** It does not, and 148 MB is not something to fetch without asking. */
    public data object NeedsModel : VoiceState

    public data class Downloading(val percent: Int) : VoiceState

    public data class Recording(val startedAtMillis: Long) : VoiceState

    public data object Transcribing : VoiceState

    public data class Failed(val message: String) : VoiceState
}

/** The daemon calls a voice job needs, narrow enough to fake in a test. */
public interface VoiceTransport {
    public suspend fun voiceModelAvailable(): Boolean

    public suspend fun downloadVoiceModel(token: String)

    public suspend fun startVoiceStream(token: String)

    public suspend fun streamVoiceChunk(token: String, pcm: ByteArray)

    public suspend fun finishVoiceStream(token: String, wav: ByteArray)

    public suspend fun cancelVoice(token: String)
}

/**
 * One recording, from tapping the microphone to text in the composer.
 *
 * The phone captures audio and the daemon does everything after that: it owns
 * the speech model and the whisper binary, and a phone should not be running a
 * 148 MB model or holding it on disk. So this is mostly a state machine over
 * one request and the `voice` events that answer it.
 *
 * The daemon addresses those events to the connection that asked, and a
 * reconnect is a new connection — so a job in flight when the socket drops is
 * lost rather than resumed. The desktop behaves the same way.
 *
 * Every field here is touched only from [scope]'s dispatcher, which is the
 * main thread on both platforms.
 */
public class VoiceSession internal constructor(
    private val transport: VoiceTransport,
    private val recorders: () -> VoiceRecorder,
    private val scope: CoroutineScope,
    private val onTranscript: (String) -> Unit,
    private val nowMillis: () -> Long,
    private val newToken: () -> String,
) {
    private val _state = MutableStateFlow<VoiceState>(VoiceState.Idle)
    public val state: StateFlow<VoiceState> = _state.asStateFlow()
    private val _partial = MutableStateFlow("")
    public val partial: StateFlow<String> = _partial.asStateFlow()
    private val _handsFree = MutableStateFlow(false)

    /** Whether each pause sends what was said and opens the microphone again. */
    public val handsFree: StateFlow<Boolean> = _handsFree.asStateFlow()

    /** Bumped by anything that abandons a job, so late results are ignored. */
    private var generation = 0L
    private var token: String? = null

    /**
     * One recorder per recording, rather than one for the session.
     *
     * Stop and cancel are signals to a running capture, and a fresh object per
     * recording is what keeps a signal that arrives a moment early -- before
     * the capture coroutine is even scheduled -- from being read by the *next*
     * recording instead.
     */
    private var recorder: VoiceRecorder? = null

    public constructor(
        transport: VoiceTransport,
        recorders: () -> VoiceRecorder,
        scope: CoroutineScope,
        onTranscript: (String) -> Unit,
    ) : this(
        transport = transport,
        recorders = recorders,
        scope = scope,
        onTranscript = onTranscript,
        nowMillis = ::currentEpochMillis,
        newToken = ::randomVoiceToken,
    )

    /**
     * Starts a recording, checking the daemon has a speech model first.
     *
     * Asking costs a round trip before the microphone opens, but recording
     * into a daemon that cannot transcribe wastes the reader's words.
     */
    public fun start() {
        val current = _state.value
        if (current !is VoiceState.Idle && current !is VoiceState.Failed) return

        val mark = ++generation
        token = newToken()
        _state.value = VoiceState.Checking
        scope.launch {
            val available = try {
                transport.voiceModelAvailable()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                fail(mark, error.message ?: "Could not reach the daemon")
                return@launch
            }
            if (mark != generation) return@launch
            if (available) beginRecording(mark) else _state.value = VoiceState.NeedsModel
        }
    }

    /** Agrees to the model download. Progress arrives as events. */
    public fun confirmDownload() {
        if (_state.value !is VoiceState.NeedsModel) return
        val request = token ?: return
        val mark = generation
        _state.value = VoiceState.Downloading(0)
        scope.launch {
            try {
                transport.downloadVoiceModel(request)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                fail(mark, error.message ?: "The speech model could not be downloaded")
            }
        }
    }

    /**
     * Turns hands-free dictation on or off.
     *
     * On, this starts listening at once and keeps going: each pause ends an
     * utterance, which is transcribed, handed over, and followed by the next
     * recording. Off cancels whatever is in flight rather than letting it land
     * -- someone switching this off is asking not to be listened to, and the
     * half-sentence they were saying is not a message they meant to send.
     */
    public fun setHandsFree(on: Boolean) {
        if (_handsFree.value == on) return
        _handsFree.value = on
        if (on) start() else cancel()
    }

    /** Ends the recording and sends it to be transcribed. */
    public fun stop() {
        if (_state.value !is VoiceState.Recording) return
        _state.value = VoiceState.Transcribing
        recorder?.stop()
    }

    /** Abandons whatever is running, telling the daemon if it has work. */
    public fun cancel() {
        val request = token
        val daemonJob = _state.value !is VoiceState.Idle &&
            _state.value !is VoiceState.NeedsModel
        // Nothing restarts the loop except a finished utterance, so leaving the
        // flag set here would leave hands-free on and deaf. Discarding what is
        // being said is a way of saying stop.
        _handsFree.value = false
        reset()
        recorder?.cancel()
        recorder = null
        if (request != null && daemonJob) {
            scope.launch { runCatching { transport.cancelVoice(request) } }
        }
    }

    public fun dismissError() {
        if (_state.value is VoiceState.Failed) _state.value = VoiceState.Idle
    }

    /**
     * Reports a failure the platform found before the daemon was involved --
     * a refused microphone permission, say -- so one place renders every way
     * voice input can go wrong.
     */
    public fun reportFailure(message: String) {
        fail(generation, message)
    }

    public fun onEvent(event: VoiceEvent) {
        if (event.token != token) return
        when (event) {
            is VoiceEvent.Downloading -> {
                val current = _state.value
                // Progress only moves forward: a reordered frame must not
                // rewind a bar the reader is watching.
                if (current is VoiceState.Downloading && event.percent > current.percent) {
                    _state.value = VoiceState.Downloading(event.percent)
                }
            }
            is VoiceEvent.Ready ->
                if (_state.value is VoiceState.Downloading) beginRecording(generation)
            is VoiceEvent.Partial ->
                if (_state.value is VoiceState.Recording || _state.value is VoiceState.Transcribing) {
                    _partial.value = event.text
                }
            is VoiceEvent.Transcribed ->
                if (_state.value is VoiceState.Transcribing) {
                    reset()
                    onTranscript(event.text)
                    // The whole point of hands-free: the next thing said is
                    // caught without anyone reaching for the phone.
                    if (_handsFree.value) start()
                }
            is VoiceEvent.Cancelled -> reset()
            is VoiceEvent.Failed -> fail(generation, event.message)
        }
    }

    private fun beginRecording(mark: Long) {
        scope.launch {
            val request = token ?: return@launch
            try {
                transport.startVoiceStream(request)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                fail(mark, error.message ?: "Could not start voice recognition")
                return@launch
            }
            if (mark != generation) return@launch

            val active = recorders()
            recorder = active
            _partial.value = ""
            _state.value = VoiceState.Recording(nowMillis())
            val chunks = Channel<ByteArray>(Channel.UNLIMITED)
            var uploadFailure: Throwable? = null
            val uploader = launch {
                try {
                    for (chunk in chunks) {
                        if (mark != generation) break
                        transport.streamVoiceChunk(request, chunk)
                    }
                } catch (error: CancellationException) {
                    throw error
                } catch (error: Throwable) {
                    uploadFailure = error
                    active.cancel()
                }
            }
            // Only the recorder is signalled from the capture thread. Ending
            // the utterance any harder would touch state that belongs to the
            // scope's dispatcher; `record` returning does the rest below.
            val listener = if (_handsFree.value) EndOfSpeech() else null
            val pcm = try {
                active.record { chunk ->
                    chunks.trySend(chunk)
                    if (listener?.accept(chunk) == true) active.stop()
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                runCatching { transport.cancelVoice(request) }
                fail(mark, error.message ?: "The microphone is not available")
                return@launch
            } finally {
                chunks.close()
            }
            uploader.join()
            if (recorder === active) recorder = null
            if (mark != generation) return@launch
            uploadFailure?.let {
                runCatching { transport.cancelVoice(request) }
                fail(mark, it.message ?: "Could not stream voice audio")
                return@launch
            }
            if (pcm.size < Wav.MIN_PCM_BYTES) {
                runCatching { transport.cancelVoice(request) }
                fail(mark, "That was too short to transcribe")
                return@launch
            }
            _state.value = VoiceState.Transcribing
            try {
                transport.finishVoiceStream(request, Wav.fromPcm16(pcm))
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                runCatching { transport.cancelVoice(request) }
                fail(mark, error.message ?: "The recording could not be transcribed")
            }
        }
    }

    private fun fail(mark: Long, message: String) {
        if (mark != generation) return
        reset()
        // A hands-free loop that restarts through failures is a loop that spins
        // on a broken microphone and buries the reason under the next attempt.
        // Drop out and let the error be read.
        _handsFree.value = false
        _state.value = VoiceState.Failed(message)
    }

    private fun reset() {
        generation++
        token = null
        _partial.value = ""
        _state.value = VoiceState.Idle
    }
}

/**
 * A correlation token for one voice job.
 *
 * Not a secret and not security-relevant: the daemon keys jobs on the
 * connection as well as the token, so this only has to be unique among one
 * client's own outstanding requests.
 */
internal fun randomVoiceToken(): String = buildString {
    repeat(2) {
        append(Random.nextLong().toULong().toString(16).padStart(16, '0'))
    }
}
