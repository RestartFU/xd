package com.restartfu.xd.mobile

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import com.restartfu.xd.voice.VoiceRecorder
import com.restartfu.xd.voice.VoiceRecorderException
import com.restartfu.xd.voice.Wav
import java.io.ByteArrayOutputStream
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Microphone capture straight into the format the daemon wants.
 *
 * `AudioRecord` rather than `MediaRecorder`: the daemon takes 16 kHz mono PCM
 * in a WAV, and MediaRecorder only produces compressed containers that would
 * have to be decoded again on the other side.
 *
 * One instance records once. [VoiceSession] makes a new one per recording so a
 * stop or cancel cannot apply to the wrong capture.
 */
class AndroidVoiceRecorder : VoiceRecorder {
    private val stopped = AtomicBoolean(false)
    private val cancelled = AtomicBoolean(false)

    // The permission is checked before a recording is ever started; Compose
    // asks for it and will not call this without it.
    @SuppressLint("MissingPermission")
    override suspend fun record(): ByteArray = withContext(Dispatchers.IO) {
        val minimum = AudioRecord.getMinBufferSize(Wav.SAMPLE_RATE, CHANNEL, ENCODING)
        if (minimum <= 0) {
            throw VoiceRecorderException("This device cannot record at 16 kHz")
        }
        // Twice the minimum, so a scheduling hiccup drops nothing.
        val bufferBytes = minimum * 2

        val recorder = try {
            AudioRecord(
                // Tuned for speech recognition rather than a phone call, which
                // is what this audio is actually for.
                MediaRecorder.AudioSource.VOICE_RECOGNITION,
                Wav.SAMPLE_RATE,
                CHANNEL,
                ENCODING,
                bufferBytes,
            )
        } catch (error: SecurityException) {
            throw VoiceRecorderException("Microphone access was refused", error)
        } catch (error: IllegalArgumentException) {
            throw VoiceRecorderException("This device cannot record at 16 kHz", error)
        }

        try {
            if (recorder.state != AudioRecord.STATE_INITIALIZED) {
                throw VoiceRecorderException("The microphone is not available")
            }
            recorder.startRecording()
            capture(recorder, bufferBytes)
        } finally {
            runCatching { recorder.stop() }
            recorder.release()
        }
    }

    override fun stop() {
        stopped.set(true)
    }

    override fun cancel() {
        cancelled.set(true)
    }

    private fun capture(recorder: AudioRecord, bufferBytes: Int): ByteArray {
        val captured = ByteArrayOutputStream()
        val buffer = ByteArray(bufferBytes)

        // read() blocks until the buffer fills, so a stop takes effect within
        // one buffer -- about a tenth of a second, which nobody notices.
        while (!stopped.get() && !cancelled.get() && captured.size() < Wav.MAX_PCM_BYTES) {
            val read = recorder.read(buffer, 0, buffer.size)
            if (read < 0) throw VoiceRecorderException(describe(read))
            val room = Wav.MAX_PCM_BYTES - captured.size()
            captured.write(buffer, 0, minOf(read, room))
        }
        if (cancelled.get()) return ByteArray(0)

        val bytes = captured.toByteArray()
        // ENCODING_PCM_16BIT is little-endian on every ABI Android ships, which
        // is what WAV stores, so the samples travel as they are. A partial
        // frame would not be a sample at all, so it is dropped.
        return if (bytes.size % 2 == 0) bytes else bytes.copyOf(bytes.size - 1)
    }

    private fun describe(code: Int): String = when (code) {
        AudioRecord.ERROR_INVALID_OPERATION -> "The microphone stopped unexpectedly"
        AudioRecord.ERROR_BAD_VALUE -> "The microphone returned unusable audio"
        AudioRecord.ERROR_DEAD_OBJECT -> "Another app took the microphone"
        else -> "The microphone could not be read"
    }

    private companion object {
        const val CHANNEL = AudioFormat.CHANNEL_IN_MONO
        const val ENCODING = AudioFormat.ENCODING_PCM_16BIT
    }
}
