package com.restartfu.xd.voice

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertTrue
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.test.runTest

private const val CHUNK_MILLIS = 100

private fun chunk(amplitude: Int): ByteArray {
    val samples = Wav.SAMPLE_RATE * CHUNK_MILLIS / 1_000
    val pcm = ByteArray(samples * 2)
    for (sample in 0 until samples) {
        val value = if (sample % 2 == 0) amplitude else -amplitude
        pcm[sample * 2] = (value and 0xFF).toByte()
        pcm[sample * 2 + 1] = ((value shr 8) and 0xFF).toByte()
    }
    return pcm
}

/** Someone saying something and then stopping, with room left over. */
private fun utterance(): List<ByteArray> =
    List(10) { chunk(9_000) } + List(40) { chunk(0) }

private class FakeVoiceTransport : VoiceTransport {
    val started = mutableListOf<String>()
    val finished = mutableListOf<ByteArray>()
    val cancelled = mutableListOf<String>()

    override suspend fun voiceModelAvailable(): Boolean = true

    override suspend fun downloadVoiceModel(token: String) = Unit

    override suspend fun startVoiceStream(token: String) {
        started += token
    }

    override suspend fun streamVoiceChunk(token: String, pcm: ByteArray) = Unit

    override suspend fun finishVoiceStream(token: String, wav: ByteArray) {
        finished += wav
    }

    override suspend fun cancelVoice(token: String) {
        cancelled += token
    }
}

/**
 * A recorder that plays a fixed recording and then holds the microphone open,
 * so a stop can arrive from inside the chunk callback -- which is where
 * hands-free ends an utterance -- or from outside, which is the button.
 */
private class ScriptedRecorder(private val script: List<ByteArray>) : VoiceRecorder {
    private val open = CompletableDeferred<Unit>()
    private var stopped = false
    private var discarded = false
    val delivered = mutableListOf<ByteArray>()
    var cancels = 0

    override suspend fun record(onChunk: (ByteArray) -> Unit): ByteArray {
        for (piece in script) {
            if (stopped) break
            delivered += piece
            onChunk(piece)
        }
        if (!stopped) open.await()
        if (discarded) return ByteArray(0)
        var pcm = ByteArray(0)
        for (piece in delivered) pcm += piece
        return pcm
    }

    override fun stop() {
        stopped = true
        open.complete(Unit)
    }

    override fun cancel() {
        cancels++
        discarded = true
        stopped = true
        open.complete(Unit)
    }
}

private fun CoroutineScope.handsFreeSession(
    transport: VoiceTransport,
    recorders: () -> VoiceRecorder,
    onTranscript: (String) -> Unit = {},
): VoiceSession = VoiceSession(
    transport = transport,
    recorders = recorders,
    scope = this,
    onTranscript = onTranscript,
    nowMillis = { 0L },
    newToken = { "token" },
)

class HandsFreeTest {
    @Test
    fun a_pause_ends_the_utterance_without_anyone_pressing_stop() = runTest {
        val transport = FakeVoiceTransport()
        val recorders = mutableListOf<ScriptedRecorder>()
        val voice = handsFreeSession(transport, {
            ScriptedRecorder(utterance()).also { recorders += it }
        })

        voice.setHandsFree(true)
        testScheduler.runCurrent()

        // It stopped part-way through the trailing silence rather than running
        // to the end of what the microphone had.
        val recorded = recorders.single()
        assertTrue(
            recorded.delivered.size < utterance().size,
            "the pause should have ended it: ${recorded.delivered.size} chunks",
        )
        assertEquals(1, transport.finished.size)
        assertIs<VoiceState.Transcribing>(voice.state.value)
    }

    @Test
    fun the_next_utterance_is_caught_without_reaching_for_the_phone() = runTest {
        val transport = FakeVoiceTransport()
        val transcripts = mutableListOf<String>()
        val voice = handsFreeSession(
            transport,
            { ScriptedRecorder(utterance()) },
        ) { transcripts += it }

        voice.setHandsFree(true)
        testScheduler.runCurrent()
        voice.onEvent(VoiceEvent.Transcribed("token", "rename the parser"))
        testScheduler.runCurrent()

        assertEquals(listOf("rename the parser"), transcripts)
        // A second stream opened and a second utterance ran through it, with no
        // second tap anywhere.
        assertEquals(2, transport.started.size)
        assertEquals(2, transport.finished.size)
        assertTrue(voice.handsFree.value)
        voice.cancel()
    }

    @Test
    fun one_recording_still_ends_at_the_button_when_hands_free_is_off() = runTest {
        val transport = FakeVoiceTransport()
        val recorders = mutableListOf<ScriptedRecorder>()
        val voice = handsFreeSession(transport, {
            ScriptedRecorder(utterance()).also { recorders += it }
        })

        voice.start()
        testScheduler.runCurrent()

        // The same silent tail, recorded rather than acted on: a reader who is
        // thinking has not finished dictating.
        assertEquals(utterance().size, recorders.single().delivered.size)
        assertIs<VoiceState.Recording>(voice.state.value)
        assertEquals(0, transport.finished.size)
        voice.cancel()
    }

    @Test
    fun switching_it_off_stops_listening_and_drops_what_was_being_said() = runTest {
        val transport = FakeVoiceTransport()
        val recorders = mutableListOf<ScriptedRecorder>()
        val voice = handsFreeSession(transport, {
            ScriptedRecorder(List(200) { chunk(9_000) }).also { recorders += it }
        })

        voice.setHandsFree(true)
        testScheduler.runCurrent()
        voice.setHandsFree(false)
        testScheduler.runCurrent()

        assertFalse(voice.handsFree.value)
        assertEquals(VoiceState.Idle, voice.state.value)
        assertEquals(1, recorders.single().cancels)
        assertEquals(listOf("token"), transport.cancelled)
    }

    @Test
    fun a_failure_drops_out_of_hands_free_rather_than_spinning_on_it() = runTest {
        val transport = FakeVoiceTransport()
        val voice = handsFreeSession(transport, { ScriptedRecorder(utterance()) })

        voice.setHandsFree(true)
        testScheduler.runCurrent()
        voice.onEvent(VoiceEvent.Failed("token", "The microphone is not available"))
        testScheduler.runCurrent()

        assertFalse(voice.handsFree.value, "a broken microphone must not be retried forever")
        assertIs<VoiceState.Failed>(voice.state.value)
    }
}
