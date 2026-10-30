package com.restartfu.xd.files

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

private fun OpenFiles.names(): List<String> = files.map { it.name }

class OpenFilesTest {
    @Test
    fun the_tree_is_where_it_starts_and_where_closing_the_last_file_lands() {
        var open = OpenFiles()
        assertEquals(FileTab.Tree, open.active)
        assertNull(open.current)

        open = open.opened("src/Wav.kt", "fun wav() {}")
        assertEquals(FileTab.File("src/Wav.kt"), open.active)
        assertEquals("fun wav() {}", open.current?.text)

        open = open.closed("src/Wav.kt")
        assertEquals(FileTab.Tree, open.active)
        assertEquals(emptyList(), open.names())
    }

    @Test
    fun opening_the_same_file_twice_is_the_same_tab() {
        val open = OpenFiles()
            .opened("a.kt", "one")
            .opened("b.kt", "two")
            .edited("a.kt", "one, edited")
            .opened("a.kt", "one")

        assertEquals(listOf("a.kt", "b.kt"), open.names())
        assertEquals(FileTab.File("a.kt"), open.active)
        // Reopening must not throw away what was typed into the tab already
        // there: two tabs on one path would be two sets of edits to reconcile.
        assertEquals("one, edited", open.current?.text)
    }

    @Test
    fun closing_lands_on_the_tab_to_the_left() {
        val open = OpenFiles()
            .opened("a.kt", "")
            .opened("b.kt", "")
            .opened("c.kt", "")
            .closed("c.kt")

        assertEquals(FileTab.File("b.kt"), open.active)
        assertEquals(listOf("a.kt", "b.kt"), open.names())

        // Closing the leftmost has nothing to its left, so it lands right.
        assertEquals(FileTab.File("b.kt"), open.closed("a.kt").active)
    }

    @Test
    fun closing_a_tab_that_is_not_in_front_leaves_the_front_alone() {
        val open = OpenFiles()
            .opened("a.kt", "")
            .opened("b.kt", "")
            .showing(FileTab.Tree)
            .closed("a.kt")

        assertEquals(FileTab.Tree, open.active)
        assertEquals(listOf("b.kt"), open.names())
    }

    @Test
    fun a_file_is_dirty_only_while_it_differs_from_what_was_written() {
        var open = OpenFiles().opened("a.kt", "one")
        assertFalse(open.anyDirty)

        open = open.edited("a.kt", "two")
        assertTrue(open.anyDirty)
        assertTrue(open.current!!.dirty)

        open = open.saving("a.kt").saved("a.kt", "two")
        assertFalse(open.anyDirty)
        assertFalse(open.current!!.saving)
    }

    @Test
    fun typing_during_a_save_is_not_marked_as_already_written() {
        var open = OpenFiles().opened("a.kt", "one").edited("a.kt", "two").saving("a.kt")
        // The round trip is in flight and the reader keeps typing.
        open = open.edited("a.kt", "two and three")
        open = open.saved("a.kt", "two")

        // "two" reached the host; "two and three" did not, and is still unsaved.
        assertEquals("two", open.current?.saved)
        assertEquals("two and three", open.current?.text)
        assertTrue(open.anyDirty)
    }

    @Test
    fun a_failed_write_keeps_the_edit_and_says_why() {
        val open = OpenFiles()
            .opened("a.kt", "one")
            .edited("a.kt", "two")
            .saving("a.kt")
            .failed("a.kt", "The file changed on disk")

        assertEquals("two", open.current?.text)
        assertEquals("one", open.current?.saved)
        assertFalse(open.current!!.saving)
        assertEquals("The file changed on disk", open.current?.error)
        assertTrue(open.anyDirty)
    }
}
