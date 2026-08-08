package com.restartfu.xd.voice

import kotlin.math.max
import kotlin.math.pow
import kotlin.math.sqrt

/**
 * Where one spoken utterance ends, so hands-free dictation knows when to send.
 *
 * Reads the same 16 kHz mono PCM16 chunks the recorder already streams to the
 * daemon, so hands-free costs no extra capture and no protocol change. The
 * daemon's partial transcripts cannot do this job: they say what was heard, not
 * that the speaker has stopped, and they arrive too late to end a turn on.
 *
 * The threshold is relative to the loudest speech heard rather than absolute,
 * because a phone's gain and a room's noise floor are both unknown here: what
 * counts as silence is a fraction of how loud this speaker actually is. That
 * peak decays, so one slammed door does not leave the detector deaf to a normal
 * voice for the rest of the utterance.
 *
 * Silence before anyone has spoken is not the end of anything -- it is somebody
 * thinking -- so [leadInMillis] of speech has to land before a pause can close
 * the utterance.
 */
public class EndOfSpeech(
    private val leadInMillis: Int = LEAD_IN_MILLIS,
    private val silenceMillis: Int = SILENCE_MILLIS,
) {
    private var loudest = 0.0
    private var spokenBytes = 0
    private var quietBytes = 0
    private var ended = false

    /**
     * Takes the next chunk of the recording.
     *
     * @return true once the utterance is over, and true for every chunk after
     *   that: a detector reports one ending, not a new one per chunk.
     */
    public fun accept(pcm: ByteArray): Boolean {
        if (ended) return true
        val level = amplitude(pcm)
        loudest = max(level, loudest * DECAY_PER_SECOND.pow(seconds(pcm.size)))
        val speaking = loudest >= FLOOR && level >= loudest * QUIET_FRACTION
        if (speaking) {
            spokenBytes += pcm.size
            quietBytes = 0
        } else if (spokenBytes >= bytes(leadInMillis)) {
            quietBytes += pcm.size
            if (quietBytes >= bytes(silenceMillis)) ended = true
        }
        return ended
    }

    /** Whether anything has been said yet, for a caller deciding to discard. */
    public fun heardSpeech(): Boolean = spokenBytes >= bytes(leadInMillis)

    public companion object {
        /** Speech needed before a pause can end the utterance. */
        public const val LEAD_IN_MILLIS: Int = 300

        /** Pause that ends it. Long enough to think mid-sentence. */
        public const val SILENCE_MILLIS: Int = 1_500

        /**
         * Quiet in absolute terms, out of a full-scale 32768. Below this the
         * loudest thing heard is room noise, and nothing has been said.
         */
        private const val FLOOR: Double = 350.0

        /** Of the loudest speech so far, under which a chunk is a pause. */
        private const val QUIET_FRACTION: Double = 0.10

        /** How much of the peak survives a second, so it tracks the speaker. */
        private const val DECAY_PER_SECOND: Double = 0.4

        private const val BYTES_PER_SAMPLE: Int = 2

        private fun bytes(millis: Int): Int =
            Wav.SAMPLE_RATE * BYTES_PER_SAMPLE * millis / 1_000

        private fun seconds(byteCount: Int): Double =
            byteCount.toDouble() / (Wav.SAMPLE_RATE * BYTES_PER_SAMPLE)

        /** Root mean square of the chunk's samples, as a 16-bit magnitude. */
        internal fun amplitude(pcm: ByteArray): Double {
            val samples = pcm.size / BYTES_PER_SAMPLE
            if (samples == 0) return 0.0
            var sum = 0.0
            var at = 0
            repeat(samples) {
                val low = pcm[at].toInt() and 0xFF
                val high = pcm[at + 1].toInt()
                val sample = ((high shl 8) or low).toShort().toDouble()
                sum += sample * sample
                at += BYTES_PER_SAMPLE
            }
            return sqrt(sum / samples)
        }
    }
}
