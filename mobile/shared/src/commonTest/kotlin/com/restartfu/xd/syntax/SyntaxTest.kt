package com.restartfu.xd.syntax

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class SyntaxTest {
    @Test
    fun resolvesLanguagesByNameBeforeExtension() {
        assertEquals(SyntaxLanguage.DOCKERFILE, Syntax.languageForPath("Dockerfile"))
        assertEquals(SyntaxLanguage.DOCKERFILE, Syntax.languageForPath("a/Dockerfile.dev"))
        assertEquals(SyntaxLanguage.MAKEFILE, Syntax.languageForPath("GNUmakefile"))
        assertEquals(SyntaxLanguage.RUBY, Syntax.languageForPath("app/Gemfile"))
        assertEquals(SyntaxLanguage.BASH, Syntax.languageForPath("home/.bashrc"))
    }

    @Test
    fun resolvesLanguagesByExtension() {
        assertEquals(SyntaxLanguage.CRYSTAL, Syntax.languageForPath("src/example.cr"))
        assertEquals(SyntaxLanguage.KOTLIN, Syntax.languageForPath("a\\b\\Main.kt"))
        assertEquals(SyntaxLanguage.C, Syntax.languageForPath("x.h"))
        assertEquals(SyntaxLanguage.NONE, Syntax.languageForPath("notes.txt"))
        assertEquals(SyntaxLanguage.NONE, Syntax.languageForPath("README"))
        assertEquals(SyntaxLanguage.NONE, Syntax.languageForPath(null))
    }

    @Test
    fun mapsMarkdownFenceTags() {
        assertEquals(SyntaxLanguage.BASH, Syntax.languageForFence("sh"))
        assertEquals(SyntaxLanguage.CRYSTAL, Syntax.languageForFence("crystal"))
        assertEquals(SyntaxLanguage.KOTLIN, Syntax.languageForFence("kotlin title=x"))
        assertEquals(SyntaxLanguage.NONE, Syntax.languageForFence(""))
        assertEquals(SyntaxLanguage.NONE, Syntax.languageForFence("brainfuck"))
    }

    @Test
    fun colouringMatchesTheDesktopPalette() {
        assertEquals("#dc8add", SyntaxToken.KEYWORD.colour)
        assertEquals("#f8e45c", SyntaxToken.STRING.colour)
        assertEquals("#8b8e8f", SyntaxToken.COMMENT.colour)
        assertEquals(null, SyntaxToken.TEXT.colour)
    }

    @Test
    fun anUnknownLanguageStaysOneRunOfPlainText() {
        val pieces = Syntax.scanLine(SyntaxLanguage.NONE, "anything at all", SyntaxState())

        assertEquals(listOf(SyntaxPiece(SyntaxToken.TEXT, "anything at all")), pieces)
    }

    @Test
    fun coloursKeywordsTypesAndCalls() {
        val pieces = Syntax.scanLine(SyntaxLanguage.GO, "func main() { var x int }", SyntaxState())

        assertEquals(SyntaxToken.KEYWORD, pieces.tokenOf("func"))
        assertEquals(SyntaxToken.FUNCTION, pieces.tokenOf("main"))
        assertEquals(SyntaxToken.TYPE, pieces.tokenOf("int"))
    }

    @Test
    fun coloursStringsNumbersAndComments() {
        val pieces = Syntax.scanLine(SyntaxLanguage.C, """int n = 42; // note""", SyntaxState())

        assertEquals(SyntaxToken.NUMBER, pieces.tokenOf("42"))
        assertEquals(SyntaxToken.COMMENT, pieces.tokenOf("// note"))

        val string = Syntax.scanLine(SyntaxLanguage.C, """char *s = "hi";""", SyntaxState())
        assertEquals(SyntaxToken.STRING, string.tokenOf("\"hi\""))
    }

    @Test
    fun anEscapedQuoteDoesNotEndAString() {
        // The line is:  s := "a\"b" + c
        val line = "s := \"a\\\"b\" + c"

        val pieces = Syntax.scanLine(SyntaxLanguage.GO, line, SyntaxState())

        assertEquals(SyntaxToken.STRING, pieces.tokenOf("\"a\\\"b\""))
    }

    @Test
    fun aBlockCommentSurvivesUntilItCloses() {
        val state = SyntaxState()

        val opened = Syntax.scanLine(SyntaxLanguage.C, "int a; /* start", state)
        assertEquals(SyntaxToken.COMMENT, opened.tokenOf("/* start"))
        assertTrue(state.inComment > 0)

        val middle = Syntax.scanLine(SyntaxLanguage.C, "still comment", state)
        assertEquals(listOf(SyntaxPiece(SyntaxToken.COMMENT, "still comment")), middle)

        val closed = Syntax.scanLine(SyntaxLanguage.C, "done */ int b;", state)
        assertEquals(SyntaxToken.COMMENT, closed.tokenOf("done */"))
        assertEquals(0, state.inComment)
        assertEquals(SyntaxToken.KEYWORD, closed.tokenOf("int"))
    }

    @Test
    fun rustBlockCommentsNest() {
        val state = SyntaxState()

        Syntax.scanLine(SyntaxLanguage.RUST, "/* outer /* inner */", state)
        assertTrue(state.inComment > 0)

        Syntax.scanLine(SyntaxLanguage.RUST, "*/ let x = 1;", state)
        assertEquals(0, state.inComment)
    }

    @Test
    fun aGoRawStringSpansLines() {
        val state = SyntaxState()

        val opened = Syntax.scanLine(SyntaxLanguage.GO, "s := `start", state)
        assertEquals(SyntaxToken.STRING, opened.tokenOf("`start"))
        assertTrue(state.inRawString)

        val closed = Syntax.scanLine(SyntaxLanguage.GO, "end` + x", state)
        assertEquals(SyntaxToken.STRING, closed.tokenOf("end`"))
    }

    @Test
    fun aKotlinTripleStringSpansLines() {
        val state = SyntaxState()

        Syntax.scanLine(SyntaxLanguage.KOTLIN, "val s = \"\"\"open", state)
        assertTrue(state.inTripleString)

        Syntax.scanLine(SyntaxLanguage.KOTLIN, "close\"\"\"", state)
        assertTrue(!state.inTripleString)
    }

    @Test
    fun hashCommentsFollowEachLanguagesRule() {
        // YAML needs the hash preceded by space; Dockerfile needs line start.
        assertEquals(
            SyntaxToken.COMMENT,
            Syntax.scanLine(SyntaxLanguage.YAML, "key: v # note", SyntaxState()).tokenOf("# note"),
        )
        assertEquals(
            SyntaxToken.COMMENT,
            Syntax.scanLine(SyntaxLanguage.DOCKERFILE, "  # note", SyntaxState()).tokenOf("# note"),
        )
        assertEquals(
            SyntaxToken.COMMENT,
            Syntax.scanLine(SyntaxLanguage.BASH, "ls # note", SyntaxState()).tokenOf("# note"),
        )
    }

    @Test
    fun aShebangIsAComment() {
        val pieces = Syntax.scanLine(SyntaxLanguage.BASH, "#!/usr/bin/env bash", SyntaxState())

        assertEquals(listOf(SyntaxPiece(SyntaxToken.COMMENT, "#!/usr/bin/env bash")), pieces)
    }

    @Test
    fun everyPieceTogetherReproducesTheLine() {
        val lines = listOf(
            "func main() { fmt.Println(\"hi\", 1.5e-3) } // done",
            "int x = 0xFF; /* a */ char c = 'y';",
            "key: [1, true] # note",
        )
        val languages = listOf(SyntaxLanguage.GO, SyntaxLanguage.C, SyntaxLanguage.YAML)

        lines.zip(languages).forEach { (line, language) ->
            val joined = Syntax.scanLine(language, line, SyntaxState()).joinToString("") { it.text }
            assertEquals(line, joined, "scanning must not lose or invent characters")
        }
    }

    private fun List<SyntaxPiece>.tokenOf(text: String): SyntaxToken? =
        firstOrNull { it.text == text }?.token
}
