package com.restartfu.xd.voice

import kotlin.test.Test
import kotlin.test.assertEquals

class SpeakTagVisibilityTest {
    @Test
    fun hidesCompleteSpeechMarkupAndKeepsItsBody() {
        assertEquals(
            "Before hello after",
            SpeakTagVisibility.render("Before <speak>hello</speak> after"),
        )
    }

    @Test
    fun keepsCodeExamplesLiteral() {
        val text = "`<speak>inline</speak>`\n```text\n<speak>fenced</speak>\n```"
        assertEquals(text, SpeakTagVisibility.render(text))
    }

    @Test
    fun hidesPartialTagsOnlyWhileStreaming() {
        assertEquals("Before hello", SpeakTagVisibility.render("Before <speak>hello", live = true))
        assertEquals("Before ", SpeakTagVisibility.render("Before <spe", live = true))
        assertEquals("hello", SpeakTagVisibility.render("<speak>hello</spe", live = true))
        assertEquals(
            "Before <speak>hello",
            SpeakTagVisibility.render("Before <speak>hello"),
        )
    }
}
