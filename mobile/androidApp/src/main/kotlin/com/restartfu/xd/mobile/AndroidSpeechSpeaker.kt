package com.restartfu.xd.mobile

import android.content.Context
import android.speech.tts.TextToSpeech
import java.util.ArrayDeque

/**
 * Small lifecycle-bound wrapper around Android's system text-to-speech engine.
 *
 * The engine is created only while the global speech setting is enabled. Text
 * that arrives before asynchronous engine initialization is kept briefly, and
 * stopping or shutting down always drops that queue.
 */
internal class AndroidSpeechSpeaker(context: Context) : TextToSpeech.OnInitListener {
    private val lock = Any()
    private val pending = ArrayDeque<String>()
    private val textToSpeech = TextToSpeech(context.applicationContext, this)
    private var ready = false
    private var closed = false
    private var utterance = 0L

    fun speak(text: String) {
        val normalized = text.trim()
        if (normalized.isEmpty()) return
        synchronized(lock) {
            if (closed) return
            if (ready) {
                speakNow(normalized)
            } else if (pending.size < MAX_PENDING) {
                pending.addLast(normalized)
            }
        }
    }

    fun stop() {
        synchronized(lock) {
            pending.clear()
            if (!closed && ready) textToSpeech.stop()
        }
    }

    fun shutdown() {
        synchronized(lock) {
            if (closed) return
            closed = true
            pending.clear()
            textToSpeech.stop()
            textToSpeech.shutdown()
        }
    }

    override fun onInit(status: Int) {
        synchronized(lock) {
            if (closed) return
            if (status != TextToSpeech.SUCCESS) {
                pending.clear()
                return
            }
            ready = true
            while (pending.isNotEmpty()) speakNow(pending.removeFirst())
        }
    }

    private fun speakNow(text: String) {
        val maxLength = TextToSpeech.getMaxSpeechInputLength()
        text.chunked(maxLength).forEach { part ->
            textToSpeech.speak(
                part,
                TextToSpeech.QUEUE_ADD,
                null,
                "xd-speech-${++utterance}",
            )
        }
    }

    private companion object {
        const val MAX_PENDING = 16
    }
}
