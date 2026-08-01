package com.restartfu.xd.markdown

import com.restartfu.xd.syntax.SyntaxLanguage
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class MarkdownTest {
    @Test
    fun readsHeadingsWithTheirLevel() {
        val blocks = Markdown.parse("# One\n\n### Three")

        assertEquals(1, (blocks[0] as Block.Heading).level)
        assertEquals("One", (blocks[0] as Block.Heading).spans.text())
        assertEquals(3, (blocks[1] as Block.Heading).level)
    }

    @Test
    fun stylesEmphasisStrongAndCode() {
        val spans = (Markdown.parse("a *b* **c** `d`")[0] as Block.Paragraph).spans

        assertEquals(true, spans.single { it.text == "b" }.italic)
        assertEquals(true, spans.single { it.text == "c" }.bold)
        assertEquals(true, spans.single { it.text == "d" }.code)
    }

    @Test
    fun readsGithubStrikethrough() {
        val spans = (Markdown.parse("~~gone~~")[0] as Block.Paragraph).spans

        assertEquals(true, spans.single { it.text == "gone" }.strike)
    }

    @Test
    fun keepsCodeFencesWithTheirLanguage() {
        val block = Markdown.parse("```kotlin\nval x = 1\nval y = 2\n```")[0] as Block.Code

        assertEquals("val x = 1\nval y = 2", block.text)
        assertEquals(SyntaxLanguage.KOTLIN, block.language)
    }

    @Test
    fun readsListsAndTheirStart() {
        val bullets = Markdown.parse("- one\n- two")[0] as Block.Bullets
        assertEquals(false, bullets.ordered)
        assertEquals(2, bullets.items.size)

        val ordered = Markdown.parse("3. three\n4. four")[0] as Block.Bullets
        assertEquals(true, ordered.ordered)
        assertEquals(3, ordered.start)
    }

    @Test
    fun readsBlockQuotesAsNestedBlocks() {
        val quote = Markdown.parse("> quoted")[0] as Block.Quote

        assertEquals("quoted", (quote.blocks[0] as Block.Paragraph).spans.text())
    }

    @Test
    fun readsTables() {
        val table = Markdown.parse(
            """
            | a | b |
            |---|---|
            | 1 | 2 |
            """.trimIndent(),
        ).filterIsInstance<Block.Table>().single()

        assertEquals(2, table.header.size)
        assertEquals(1, table.rows.size)
        assertEquals("1", table.rows[0][0].text().trim())
    }

    @Test
    fun readsAThematicBreak() {
        assertTrue(Markdown.parse("a\n\n---\n\nb").any { it is Block.Rule })
    }

    @Test
    fun keepsOnlyLinksThatAreSafeToFollow() {
        val safe = (Markdown.parse("[go](https://example.com)")[0] as Block.Paragraph).spans
        assertEquals("https://example.com", safe.single().link)

        // A javascript: or file: target keeps its text but loses the link, so
        // it can never be handed to the system.
        val unsafe = (Markdown.parse("[go](javascript:alert(1))")[0] as Block.Paragraph).spans
        assertEquals("go", unsafe.text())
        assertNull(unsafe.single().link)

        assertNull(Markdown.safeLink("file:///etc/passwd"))
        assertEquals("mailto:a@b.c", Markdown.safeLink("mailto:a@b.c"))
    }

    @Test
    fun showsAnImageAsItsAltText() {
        val spans = (Markdown.parse("![a diagram](https://x/y.png)")[0] as Block.Paragraph).spans

        assertEquals("Image: a diagram", spans.text())
    }

    @Test
    fun joinsSoftWrappedLines() {
        val spans = (Markdown.parse("one\ntwo")[0] as Block.Paragraph).spans

        // A newline inside a paragraph is a space, not a line break.
        assertEquals("one two", spans.text())
    }

    @Test
    fun plainProseSurvivesUnchanged() {
        val spans = (Markdown.parse("just words")[0] as Block.Paragraph).spans

        assertEquals(listOf(Span("just words")), spans)
    }

    @Test
    fun halfWrittenMarkupStillReads() {
        // Streaming shows this constantly; nothing may be lost mid-sentence.
        listOf("**bold nev", "`code nev", "[link](htt", "| a | b", "```kotlin\nval x").forEach {
            val blocks = Markdown.parse(it)
            assertTrue(blocks.isNotEmpty(), "parsing \"$it\" produced nothing")
        }
    }

    @Test
    fun emptyInputHasNoBlocks() {
        assertEquals(emptyList(), Markdown.parse(""))
        assertEquals(emptyList(), Markdown.parse("   \n  "))
    }

    private fun List<Span>.text(): String = joinToString("") { it.text }
}
