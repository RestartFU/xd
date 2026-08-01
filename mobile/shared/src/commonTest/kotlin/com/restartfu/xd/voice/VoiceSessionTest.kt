package com.restartfu.xd.voice

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertTrue
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.test.runTest

private class FakeTransport(
    var available: Boolean = true,
) : VoiceTransport {
    var failure: Throwable? = null
    val transcribed = mutableListOf<ByteArray>()
    val cancelled = mutableListOf<String>()
    var downloads = 0

    override suspend fun voiceModelAvailable(): Boolean {
        failure?.let { throw it }
        return available
    }

    override suspend fun downloadVoiceModel(token: String) {
        downloads++
    }

    override suspend fun transcribe(token: String, wav: ByteArray) {
        failure?.let { throw it }
        transcribed += wav
    }

    override suspend fun cancelVoice(token: String) {
        cancelled += token
    }
}

private class FakeRecorder(
    private val audio: ByteArray = ByteArray(Wav.MIN_PCM_BYTES),
) : VoiceRecorder {
    // One deferred for the object's life, so a cancel that arrives before
    // record() is scheduled still ends it -- which is exactly what happens
    // when a reader taps the microphone and then backs out.
    private val finished = CompletableDeferred<ByteArray>()
    var cancels = 0

    override suspend fun record(): ByteArray = finished.await()

    override fun stop() {
        finished.complete(audio)
    }

    override fun cancel() {
        cancels++
        finished.complete(ByteArray(0))
    }
}

private fun CoroutineScope.voiceSession(
    transport: VoiceTransport,
    recorder: VoiceRecorder,
    token: String = "token",
    onTranscript: (String) -> Unit = {},
): VoiceSession = VoiceSession(
    transport = transport,
    recorders = { recorder },
    scope = this,
    onTranscript = onTranscript,
    nowMillis = { 0L },
    newToken = { token },
)

class VoiceSessionTest {
    @Test
    fun recordsWhenTheDaemonAlreadyHasTheModel() = runTest {
        val transport = FakeTransport(available = true)
        val recorder = FakeRecorder()
        val transcripts = mutableListOf<String>()
        val voice = voiceSession(transport, recorder) { transcripts += it }

        voice.start()
        testScheduler.runCurrent()
        assertIs<VoiceState.Recording>(voice.state.value)

        voice.stop()
        testScheduler.runCurrent()
        assertEquals(1, transport.transcribed.size)
        // The daemon is handed a WAV, not raw samples.
        assertEquals("RIFF", transport.transcribed[0].decodeToString(0, 4))

        voice.onEvent(VoiceEvent.Transcribed("token", "make it faster"))
        assertEquals(listOf("make it faster"), transcripts)
        assertEquals(VoiceState.Idle, voice.state.value)
    }

    @Test
    fun asksBeforeDownloadingTheModelAndRecordsWhenItLands() = runTest {
        val transport = FakeTransport(available = false)
        val recorder = FakeRecorder()
        val voice = voiceSession(transport, recorder)

        voice.start()
        testScheduler.runCurrent()
        // 574 MB on someone else's machine is not a thing to start unasked.
        assertEquals(VoiceState.NeedsModel, voice.state.value)
        assertEquals(0, transport.downloads)

        voice.confirmDownload()
        testScheduler.runCurrent()
        assertEquals(1, transport.downloads)

        voice.onEvent(VoiceEvent.Downloading("token", 40))
        assertEquals(VoiceState.Downloading(40), voice.state.value)
        // A reordered frame must not rewind a bar the reader is watching.
        voice.onEvent(VoiceEvent.Downloading("token", 12))
        assertEquals(VoiceState.Downloading(40), voice.state.value)

        voice.onEvent(VoiceEvent.Ready("token"))
        assertIs<VoiceState.Recording>(voice.state.value)

        // Leave no recorder awaiting: the test scope outlives this call.
        voice.cancel()
        testScheduler.runCurrent()
    }

    @Test
    fun cancellingStopsTheRecorderAndTellsTheDaemon() = runTest {
        val transport = FakeTransport()
        val recorder = FakeRecorder()
        val voice = voiceSession(transport, recorder)

        voice.start()
        testScheduler.runCurrent()
        voice.stop()
        assertEquals(VoiceState.Transcribing, voice.state.value)

        voice.cancel()
        testScheduler.runCurrent()
        assertEquals(VoiceState.Idle, voice.state.value)
        assertEquals(listOf("token"), transport.cancelled)
        assertEquals(1, recorder.cancels)
    }

    @Test
    fun aCancelledRecordingIsNotTranscribed() = runTest {
        val transport = FakeTransport()
        val recorder = FakeRecorder()
        val voice = voiceSession(transport, recorder)

        voice.start()
        testScheduler.runCurrent()
        voice.cancel()
        testScheduler.runCurrent()

        assertTrue(transport.transcribed.isEmpty())
        // Cancelling while recording is not a daemon job, so nothing to cancel.
        assertTrue(transport.cancelled.isEmpty())
    }

    @Test
    fun refusesARecordingTooShortToTranscribe() = runTest {
        val transport = FakeTransport()
        val recorder = FakeRecorder(audio = ByteArray(200))
        val voice = voiceSession(transport, recorder)

        voice.start()
        testScheduler.runCurrent()
        voice.stop()
        testScheduler.runCurrent()

        assertTrue(transport.transcribed.isEmpty())
        assertIs<VoiceState.Failed>(voice.state.value)
    }

    @Test
    fun ignoresEventsForAnAbandonedJob() = runTest {
        val transport = FakeTransport()
        val recorder = FakeRecorder()
        val transcripts = mutableListOf<String>()
        val voice = voiceSession(transport, recorder) { transcripts += it }

        voice.start()
        testScheduler.runCurrent()
        voice.stop()
        testScheduler.runCurrent()
        voice.cancel()
        testScheduler.runCurrent()

        // The daemon may already have been transcribing when the cancel was
        // sent. Its answer must not land in a composer nobody is watching.
        voice.onEvent(VoiceEvent.Transcribed("token", "ignored"))
        assertTrue(transcripts.isEmpty())
        assertEquals(VoiceState.Idle, voice.state.value)
    }

    @Test
    fun reportsADaemonThatCannotBeReached() = runTest {
        val transport = FakeTransport()
        transport.failure = IllegalStateException("Connection is not established")
        val voice = voiceSession(transport, FakeRecorder())

        voice.start()
        testScheduler.runCurrent()

        assertEquals(
            VoiceState.Failed("Connection is not established"),
            voice.state.value,
        )
        voice.dismissError()
        assertEquals(VoiceState.Idle, voice.state.value)
    }
}
