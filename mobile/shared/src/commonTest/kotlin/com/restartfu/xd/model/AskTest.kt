package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class AskTest {
    // The same cases the Rust daemon asserts, so the phone and the daemon
    // cannot disagree about what counts as a question.
    @Test
    fun extractsTheLastValidBlock() {
        val text = """
            This explains <ask>bad</ask>.

            Choose now.

            <ask>
            Which implementation?
            - Keep parser
            * Replace parser
            - Add tests
            </ask>
        """.trimIndent()

        val parsed = requireNotNull(AskBlock.parse(text))
        assertEquals("Which implementation?", parsed.ask.question)
        assertEquals(listOf("Keep parser", "Replace parser", "Add tests"), parsed.ask.options)
        assertEquals(false, parsed.ask.acceptsInput)
        assertEquals("This explains <ask>bad</ask>.\n\nChoose now.", parsed.remainder)
    }

    @Test
    fun acceptsInputOnlyQuestionsAndJoinsMultilinePrompts() {
        val parsed = requireNotNull(
            AskBlock.parse(
                """
                Before.
                <ask>
                Which branch
                should receive this?
                <input>
                </ask>
                After.
                """.trimIndent(),
            ),
        )

        assertEquals("Which branch should receive this?", parsed.ask.question)
        assertTrue(parsed.ask.options.isEmpty())
        assertTrue(parsed.ask.acceptsInput)
        assertEquals("Before.\n\nAfter.", parsed.remainder)
    }

    @Test
    fun stripsDuplicateBlocksAndKeepsTheLastActionableAsk() {
        val text = """
            Findings.

            <ask>
            Which implementation?
            - Restore it
            - Start fresh
            </ask>

            More context.

            <ask>
            What should v1 cover?
            - Chat only
            - Full parity
            </ask>

            Closing prose.
        """.trimIndent()

        val parsed = requireNotNull(AskBlock.parse(text))
        assertEquals("What should v1 cover?", parsed.ask.question)
        assertEquals(listOf("Chat only", "Full parity"), parsed.ask.options)
        assertEquals("Findings.\n\nMore context.\n\nClosing prose.", parsed.remainder)
    }

    @Test
    fun rejectsBlocksWithoutARealChoiceOrInput() {
        assertNull(AskBlock.parse("<ask>\nQuestion?\n- Only one\n</ask>"))
        assertNull(AskBlock.parse("No question here at all"))
    }

    @Test
    fun capsChoicesAtSix() {
        val text = buildString {
            append("<ask>\nPick.\n")
            (1..8).forEach { append("- Option $it\n") }
            append("</ask>")
        }
        assertEquals(6, requireNotNull(AskBlock.parse(text)).ask.options.size)
    }

    @Test
    fun waitsOnlyWhileTheQuestionIsTheLastThingSaid() {
        val question = TranscriptItem(
            id = "1",
            kind = TranscriptKind.ASSISTANT,
            text = "<ask>\nWhich?\n- A\n- B\n</ask>",
        )
        val idle = ChatState(chatId = "chat-1", messages = listOf(question))
        assertEquals(listOf("A", "B"), AskBlock.pending(idle)?.options)

        // A turn is running, so the question has already been answered.
        assertNull(AskBlock.pending(idle.copy(working = true)))
        // Something is queued to be said next.
        assertNull(AskBlock.pending(idle.copy(queue = listOf("do it"))))
        // The reader has said something since.
        assertNull(
            AskBlock.pending(
                idle.copy(
                    messages = listOf(
                        question,
                        TranscriptItem("2", TranscriptKind.USER, "A"),
                    ),
                ),
            ),
        )
    }
}
