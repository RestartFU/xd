package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class ToolTextTest {
    @Test
    fun readsAnInlineDiffBehindTheFileChangeMarker() {
        val text = "file_change\n$PATCH"

        assertEquals(PATCH, ToolText.patch(text))
        assertEquals(listOf("src/a.kt", "src/b.kt"), ToolText.changedFiles(PATCH))
    }

    @Test
    fun treatsFileChangeWithoutAPatchAsPlainText() {
        // The daemon writes the bare marker when it could not compute a patch.
        assertNull(ToolText.patch("file_change"))
        assertNull(ToolText.patch("file_change\nnot a patch"))
        assertEquals("Edited files", ToolText.summary("file_change"))
    }

    @Test
    fun namesOneChangedFileAndCountsSeveral() {
        assertEquals(
            "only.kt",
            ToolText.summary(
                "file_change\ndiff --git a/mobile/shared/src/commonMain/kotlin/only.kt " +
                    "b/mobile/shared/src/commonMain/kotlin/only.kt\n+x",
            ),
        )
        assertEquals("2 files changed", ToolText.summary("file_change\n$PATCH"))
    }

    @Test
    fun reportsARenameByItsDestination() {
        val patch = "diff --git a/old/name.kt b/new/name.kt\nsimilarity index 100%"

        assertEquals(listOf("new/name.kt"), ToolText.changedFiles(patch))
    }

    @Test
    fun summarisesAPlainToolByItsFirstLine() {
        assertEquals("Read config.cr", ToolText.summary("Read config.cr"))
        assertEquals("Bash", ToolText.summary("Bash\n$ ls\na\nb"))
        assertEquals("Bash", ToolText.summary("\n\n  Bash  \noutput"))
    }

    @Test
    fun aOneLineToolHasNothingToExpand() {
        assertNull(ToolText.detail("Read config.cr"))
        assertNull(ToolText.detail("Read config.cr\n"))
        assertNull(ToolText.detail("Read config.cr\n   \n "))
    }

    @Test
    fun aMultiLineToolExpandsToItsWholeText() {
        assertEquals("Bash\n$ ls\na", ToolText.detail("Bash\n$ ls\na"))
    }

    @Test
    fun aDiffExpandsToItsPatch() {
        assertEquals(PATCH, ToolText.detail("file_change\n$PATCH"))
    }

    private companion object {
        val PATCH = """
            diff --git a/src/a.kt b/src/a.kt
            --- a/src/a.kt
            +++ b/src/a.kt
            @@ -1 +1 @@
            -old
            +new
            diff --git a/src/b.kt b/src/b.kt
            --- a/src/b.kt
            +++ b/src/b.kt
            @@ -1 +1 @@
            -old
            +new
        """.trimIndent()
    }
}
