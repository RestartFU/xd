package com.restartfu.xd.voice

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

class VoiceEventsTest {
    @Test
    fun readsEveryStateTheDaemonPublishes() {
        assertEquals(
            VoiceEvent.Downloading("t", 42),
            VoiceWire.event(event("downloading") { put("progress", 42) }),
        )
        assertEquals(VoiceEvent.Ready("t"), VoiceWire.event(event("ready")))
        assertEquals(
            VoiceEvent.Transcribed("t", "ship it"),
            VoiceWire.event(event("transcribed") { put("text", "ship it") }),
        )
        assertEquals(VoiceEvent.Cancelled("t"), VoiceWire.event(event("cancelled")))
        assertEquals(
            VoiceEvent.Failed("t", "whisper is not installed"),
            VoiceWire.event(event("error") { put("error", "whisper is not installed") }),
        )
    }

    @Test
    fun clampsTheProgressADownloadReportsBeforeItKnowsTheSize() {
        // VoiceJobs publishes -1 until the content length arrives, which is
        // not a percentage anything can draw.
        assertEquals(
            VoiceEvent.Downloading("t", 0),
            VoiceWire.event(event("downloading") { put("progress", -1) }),
        )
    }

    @Test
    fun treatsAMissingTranscriptAsAFailure() {
        assertEquals(
            VoiceEvent.Failed("t", "The daemon returned no transcription"),
            VoiceWire.event(event("transcribed")),
        )
    }

    @Test
    fun ignoresOtherEvents() {
        assertNull(
            VoiceWire.event(
                buildJsonObject {
                    put("event", "terminal-output")
                    put("chat", "c1")
                },
            ),
        )
        // A voice event with no token cannot be matched to a job.
        assertNull(VoiceWire.event(buildJsonObject { put("event", "voice") }))
    }

    private fun event(
        state: String,
        extra: kotlinx.serialization.json.JsonObjectBuilder.() -> Unit = {},
    ) = buildJsonObject {
        put("event", "voice")
        put("request", "t")
        put("state", state)
        extra()
    }
}
