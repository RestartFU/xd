package com.restartfu.xd.syntax

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class CodeBlocksTest {
    @Test
    fun classifiesEveryKindOfPatchLine() {
        val lines = CodeBlocks.diffLines(PATCH)

        assertEquals(DiffKind.META, lines[0].kind)
        assertEquals(DiffKind.META, lines[1].kind)
        assertEquals(DiffKind.META, lines[2].kind)
        assertEquals(DiffKind.HUNK, lines[3].kind)
        assertEquals(DiffKind.CONTEXT, lines[4].kind)
        assertEquals(DiffKind.REMOVED, lines[5].kind)
        assertEquals(DiffKind.ADDED, lines[6].kind)
    }

    @Test
    fun stripsTheMarkerSoTheScannerSeesSource() {
        val added = CodeBlocks.diffLines(PATCH).first { it.kind == DiffKind.ADDED }

        assertEquals("+", added.marker)
        assertEquals("  return 2", added.code)
    }

    @Test
    fun coloursEachFileForItsOwnLanguage() {
        val patch = """
            +++ b/main.go
            @@ -1 +1 @@
            +package main
            +++ b/x.cr
            @@ -1 +1 @@
            +puts 1
        """.trimIndent()

        val added = CodeBlocks.diffLines(patch).filter { it.kind == DiffKind.ADDED }

        assertEquals(SyntaxLanguage.GO, added[0].language)
        assertEquals(SyntaxLanguage.CRYSTAL, added[1].language)
    }

    @Test
    fun aDeletedFileHasNoLanguage() {
        val lines = CodeBlocks.diffLines("+++ /dev/null\n-gone")

        assertEquals(SyntaxLanguage.NONE, lines.last().language)
    }

    @Test
    fun splitsFencedCodeFromProse() {
        val segments = CodeBlocks.segments("before\n```kotlin\nval x = 1\n```\nafter")

        assertEquals(3, segments.size)
        assertEquals(Segment(false, "before"), segments[0])
        assertEquals(true, segments[1].code)
        assertEquals("val x = 1", segments[1].text)
        assertEquals(SyntaxLanguage.KOTLIN, segments[1].language)
        assertEquals(Segment(false, "after"), segments[2])
    }

    @Test
    fun anUnterminatedFenceStillReadsAsCode() {
        // Streaming shows a half-written block; it is code either way.
        val segments = CodeBlocks.segments("intro\n```go\nfunc main() {")

        assertEquals(2, segments.size)
        assertTrue(segments[1].code)
        assertEquals(SyntaxLanguage.GO, segments[1].language)
    }

    @Test
    fun textWithoutFencesStaysOneProseSegment() {
        val segments = CodeBlocks.segments("just words\nacross lines")

        assertEquals(listOf(Segment(false, "just words\nacross lines")), segments)
    }

    @Test
    fun tildeFencesWork() {
        val segments = CodeBlocks.segments("~~~sh\nls\n~~~")

        assertEquals(1, segments.size)
        assertTrue(segments[0].code)
        assertEquals(SyntaxLanguage.BASH, segments[0].language)
    }

    private companion object {
        val PATCH = """
            diff --git a/a.go b/a.go
            --- a/a.go
            +++ b/a.go
            @@ -1,3 +1,3 @@
             func f() int {
            -  return 1
            +  return 2
        """.trimIndent()
    }
}
