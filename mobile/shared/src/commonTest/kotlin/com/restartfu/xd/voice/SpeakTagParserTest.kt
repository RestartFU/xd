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
    fun saysAnIncompleteBlockAndDropsAWhitespaceOnlyOne() {
        val parser = SpeakTagParser()

        // Written but never closed -- by a tool call, or the end of the turn.
        // Saying it beats saying nothing.
        assertEquals(emptyList(), parser.feed("<speak>unfinished"))
        assertEquals(listOf("unfinished"), parser.finish())
        // Whitespace is not something to read out, closed or not.
        assertEquals(emptyList(), parser.feed("<speak> \n </speak>"))
        assertEquals(emptyList(), parser.finish())
    }

    @Test
    fun malformedAndNestedBlocksAreNeverSpoken() {
        val parser = SpeakTagParser()

        // A nested block is not an unambiguous speech request, and stays unsaid
        // even when the turn ends on it.
        assertEquals(emptyList(), parser.feed("<speak>one <speak>two</speak>"))
        assertEquals(emptyList(), parser.finish())
        parser.reset()
        // Nor is anything that merely looks like the tag.
        assertEquals(emptyList(), parser.feed("<spreak>not speech</spreak>"))
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

class SpeakTagStreamingTest {
    @Test
    fun a_finished_sentence_is_said_before_the_block_closes() {
        val parser = SpeakTagParser()
        // Nothing yet: the sentence has not ended.
        assertEquals(emptyList(), parser.feed("<speak>Let me look at "))
        // It has now, and waiting for the closing tag would be waiting for the
        // whole reply to be written.
        assertEquals(listOf("Let me look at the recorder."), parser.feed("the recorder. And"))
    }

    @Test
    fun a_mark_only_ends_a_sentence_once_what_follows_it_arrives() {
        val parser = SpeakTagParser()
        // A number is not a sentence: "3." must wait for the next character.
        assertEquals(emptyList(), parser.feed("<speak>It took 3."))
        assertEquals(emptyList(), parser.feed("5 seconds"))
        assertEquals(listOf("It took 3.5 seconds."), parser.feed(". "))
    }

    @Test
    fun the_tail_after_the_last_sentence_is_said_when_the_block_closes() {
        val parser = SpeakTagParser()
        assertEquals(listOf("One."), parser.feed("<speak>One.  Two"))
        assertEquals(listOf("Two"), parser.feed("</speak>"))
    }

    @Test
    fun a_block_cut_short_still_says_what_it_had() {
        val parser = SpeakTagParser()
        parser.feed("<speak>Reading the parser now")
        // A tool call or the end of the turn. It was written; silence is worse.
        assertEquals(listOf("Reading the parser now"), parser.finish())
        assertEquals(emptyList(), parser.finish())
    }

    @Test
    fun there_is_nothing_to_flush_outside_a_block() {
        val parser = SpeakTagParser()
        parser.feed("ordinary prose nobody asked to hear")
        assertEquals(emptyList(), parser.finish())
    }
}

class SpeakTagSecondBlockTest {
    @Test
    fun a_second_block_in_the_same_turn_is_spoken_too() {
        val parser = SpeakTagParser()
        assertEquals(listOf("Looking now."), parser.feed("<speak>Looking now.</speak>"))
        // Prose, then a tool, then the closing line -- one turn, two blocks.
        assertEquals(emptyList(), parser.feed("\nSome ordinary prose.\n"))
        assertEquals(emptyList(), parser.finish())
        assertEquals(listOf("Here is what I found."), parser.feed("<speak>Here is what I found.</speak>"))
    }

    @Test
    fun two_blocks_arriving_in_one_chunk_are_both_spoken() {
        val parser = SpeakTagParser()
        assertEquals(
            listOf("First.", "Second."),
            parser.feed("<speak>First.</speak> middle <speak>Second.</speak>"),
        )
    }
}
