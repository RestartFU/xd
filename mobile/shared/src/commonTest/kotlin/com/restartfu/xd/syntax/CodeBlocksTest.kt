package com.restartfu.xd.syntax

import kotlin.test.Test
import kotlin.test.assertEquals

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
    fun groupsFilesWithPathsAndChangeCounts() {
        val patch = """
            diff --git a/a.go b/a.go
            --- a/a.go
            +++ b/a.go
            @@ -1 +1,2 @@
            -old
            +new
            +another
            diff --git a/readme.md b/readme.md
            --- a/readme.md
            +++ /dev/null
            @@ -1 +0,0 @@
            -gone
        """.trimIndent()

        val files = CodeBlocks.diffFiles(patch)

        assertEquals(listOf("a.go", "readme.md"), files.map { it.path })
        assertEquals(2, files[0].additions)
        assertEquals(1, files[0].deletions)
        assertEquals(0, files[1].additions)
        assertEquals(1, files[1].deletions)
        assertEquals("--- a/readme.md", files[1].lines.first().code)
    }

    @Test
    fun supportsQuotedFilePaths() {
        val file = CodeBlocks.diffFiles(
            "diff --git \"a/a file.go\" \"b/a file.go\"\n+++ \"b/a file.go\"\n+package main",
        ).single()

        assertEquals("a file.go", file.path)
    }

    @Test
    fun keepsHeaderlessPatchesInOneSection() {
        val files = CodeBlocks.diffFiles("@@ -1 +1 @@\n-old\n+new")

        assertEquals("Changes", files.single().path)
        assertEquals(1, files.single().additions)
        assertEquals(1, files.single().deletions)
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
