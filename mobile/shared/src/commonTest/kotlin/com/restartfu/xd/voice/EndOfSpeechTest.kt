package com.restartfu.xd.voice

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** A chunk of the size the recorder streams, at a constant amplitude. */
private fun tone(amplitude: Int, millis: Int = 100): ByteArray {
    val samples = Wav.SAMPLE_RATE * millis / 1_000
    val pcm = ByteArray(samples * 2)
    for (sample in 0 until samples) {
        // Alternating sign, so the mean square is the amplitude squared rather
        // than a DC offset the detector would read as silence.
        val value = if (sample % 2 == 0) amplitude else -amplitude
        pcm[sample * 2] = (value and 0xFF).toByte()
        pcm[sample * 2 + 1] = ((value shr 8) and 0xFF).toByte()
    }
    return pcm
}

private fun feed(detector: EndOfSpeech, chunk: ByteArray, millis: Int): Boolean {
    var ended = false
    repeat(millis / 100) { ended = detector.accept(chunk) }
    return ended
}

class EndOfSpeechTest {
    @Test
    fun a_pause_after_speech_ends_the_utterance() {
        val detector = EndOfSpeech()
        assertFalse(feed(detector, tone(8_000), 1_000), "speech is not an ending")
        assertFalse(feed(detector, tone(0), 900), "a short pause is thinking")
        assertTrue(feed(detector, tone(0), 1_000), "a long pause is an ending")
    }

    @Test
    fun silence_alone_never_ends_anything() {
        val detector = EndOfSpeech()
        // Somebody who has not started talking yet is not somebody who stopped,
        // however long they take.
        assertFalse(feed(detector, tone(0), 30_000))
        assertFalse(detector.heardSpeech())
    }

    @Test
    fun room_noise_is_not_speech() {
        val detector = EndOfSpeech()
        // A quiet room drifts around a low level. Nothing here is loud enough
        // to start an utterance, so nothing can end one.
        assertFalse(feed(detector, tone(120), 5_000))
        assertFalse(detector.heardSpeech())
    }

    @Test
    fun a_pause_inside_a_sentence_does_not_cut_it_off() {
        val detector = EndOfSpeech()
        assertFalse(feed(detector, tone(9_000), 800))
        assertFalse(feed(detector, tone(0), 1_000), "still mid-sentence")
        assertFalse(feed(detector, tone(9_000), 800), "speech resumes")
        assertFalse(feed(detector, tone(0), 900), "the pause clock restarted")
        assertTrue(feed(detector, tone(0), 800))
    }

    @Test
    fun one_loud_noise_does_not_deafen_it_to_a_normal_voice() {
        val detector = EndOfSpeech()
        assertFalse(feed(detector, tone(30_000), 200), "a door slams")
        // A voice much quieter than that peak still has to register as speech,
        // or the utterance ends the moment the room settles.
        assertFalse(feed(detector, tone(4_000), 2_000))
        assertTrue(detector.heardSpeech())
        assertTrue(feed(detector, tone(0), 1_600))
    }

    @Test
    fun an_ending_is_reported_once_and_stays_reported() {
        val detector = EndOfSpeech()
        feed(detector, tone(8_000), 1_000)
        assertTrue(feed(detector, tone(0), 2_000))
        // Late chunks racing the stop must not reopen the utterance.
        assertTrue(detector.accept(tone(9_000)))
    }
}
