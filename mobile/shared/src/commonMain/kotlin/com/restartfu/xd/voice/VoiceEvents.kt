package com.restartfu.xd.voice

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull

/**
 * Progress on a voice job, already decoded off the wire.
 *
 * [token] is the request token the client chose. The daemon addresses these
 * events to the connection that asked, so the token only has to be unique
 * within one client's own outstanding jobs.
 */
public sealed interface VoiceEvent {
    public val token: String

    public data class Downloading(
        override val token: String,
        val percent: Int,
    ) : VoiceEvent

    /** The speech model is on the daemon's disk; recording may begin. */
    public data class Ready(override val token: String) : VoiceEvent

    public data class Transcribed(
        override val token: String,
        val text: String,
    ) : VoiceEvent

    public data class Partial(
        override val token: String,
        val text: String,
    ) : VoiceEvent

    public data class Cancelled(override val token: String) : VoiceEvent

    public data class Failed(
        override val token: String,
        val message: String,
    ) : VoiceEvent
}

public object VoiceWire {
    /** Reads a `voice` event, or null when this is some other event. */
    public fun event(value: JsonObject): VoiceEvent? {
        if (value.text("event") != "voice") return null
        val token = value.text("request") ?: return null
        return when (value.text("state")) {
            // A download reports -1 until the size is known, which is not a
            // percentage a progress bar can show.
            "downloading" -> VoiceEvent.Downloading(
                token,
                (value.number("progress") ?: 0).coerceIn(0, 100),
            )
            "ready" -> VoiceEvent.Ready(token)
            "transcribed" -> value.text("text")?.let { VoiceEvent.Transcribed(token, it) }
                ?: VoiceEvent.Failed(token, "The daemon returned no transcription")
            "partial" -> value.text("text")?.let { VoiceEvent.Partial(token, it) }
            "cancelled" -> VoiceEvent.Cancelled(token)
            "error" -> VoiceEvent.Failed(
                token,
                value.text("error") ?: "Voice input failed",
            )
            else -> null
        }
    }

    private fun JsonObject.text(name: String): String? =
        (this[name] as? JsonPrimitive)?.contentOrNull

    private fun JsonObject.number(name: String): Int? =
        (this[name] as? JsonPrimitive)?.takeUnless { it.isString }?.intOrNull
}
