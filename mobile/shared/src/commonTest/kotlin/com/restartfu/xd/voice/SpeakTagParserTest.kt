package com.restartfu.xd.voice

import kotlin.test.Test
import kotlin.test.assertEquals

class SpeakTagParserTest {
    @Test
    fun emitsOnlyCompleteSpeechBlocks() {
        val parser = SpeakTagParser()

        assertEquals(emptyList(), parser.feed("ordinary text"))
        assertEquals(listOf("hello"), parser.feed("<speak>hello</speak>"))
    }

    @Test
    fun handlesOpeningAndClosingTagsAcrossChunks() {
        val parser = SpeakTagParser()

        assertEquals(emptyList(), parser.feed("before <spe"))
        assertEquals(emptyList(), parser.feed("ak>hello</spe"))
        assertEquals(listOf("hello"), parser.feed("ak> after"))
    }

    @Test
    fun emitsMultipleBlocksInOneStream() {
        val parser = SpeakTagParser()

        assertEquals(
            listOf("first", "second"),
            parser.feed("<speak>first</speak> gap <speak>second</speak>"),
        )
    }

    @Test
    fun dropsIncompleteAndWhitespaceOnlyBlocksOnFinish() {
        val parser = SpeakTagParser()

        assertEquals(emptyList(), parser.feed("<speak>unfinished"))
        assertEquals(emptyList(), parser.finish())
        assertEquals(emptyList(), parser.feed("<speak> \n </speak>"))
    }

    @Test
    fun malformedAndNestedBlocksAreNeverSpoken() {
        val parser = SpeakTagParser()

        assertEquals(emptyList(), parser.feed("<speak>one <speak>two</speak>"))
        parser.reset()
        assertEquals(emptyList(), parser.feed("<spreak>not speech</spreak>"))
        assertEquals(emptyList(), parser.feed("<speak>still unfinished"))
        assertEquals(emptyList(), parser.finish())
    }

    @Test
    fun ignoresTagLikeTextInCode() {
        val parser = SpeakTagParser()

        assertEquals(
            emptyList(),
            parser.feed("`<speak>inline</speak>`\n```text\n<speak>fenced</speak>\n```"),
        )
        assertEquals(listOf("spoken"), parser.feed("<speak>spoken</speak>"))
    }

    @Test
    fun ordinaryAngleBracketsRemainSilent() {
        val parser = SpeakTagParser()

        assertEquals(emptyList(), parser.feed("2 < 3 and HTML <span>text</span>"))
    }
}
