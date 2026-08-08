package com.restartfu.xd.files

import com.restartfu.xd.protocol.FileEntryReply
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

private fun dir(name: String) = FileEntryReply(name = name, directory = true)

private fun file(name: String) = FileEntryReply(name = name, directory = false)

private fun FileTree.paths(): List<String> = rows().map { it.path }

private fun FileTree.drawn(): List<String> =
    rows().map { "${"  ".repeat(it.depth)}${it.name}${if (it.directory) "/" else ""}" }

class FileTreeTest {
    @Test
    fun a_closed_root_shows_only_its_own_entries() {
        val tree = FileTree()
            .withChildren("", listOf(dir("src"), file("README.md")))

        assertEquals(listOf("src/", "README.md"), tree.drawn())
        assertFalse(tree.isExpanded("src"))
    }

    @Test
    fun directories_sort_before_files_and_case_does_not_reorder_them() {
        val tree = FileTree().withChildren(
            "",
            listOf(file("zebra.txt"), dir("Widgets"), file("Alpha.md"), dir("apps")),
        )

        assertEquals(listOf("apps/", "Widgets/", "Alpha.md", "zebra.txt"), tree.drawn())
    }

    @Test
    fun opening_a_directory_puts_its_children_underneath_it() {
        val tree = FileTree()
            .withChildren("", listOf(dir("src"), file("README.md")))
            .toggled("src")
            .withChildren("src", listOf(dir("voice"), file("main.kt")))
            .toggled("src/voice")
            .withChildren("src/voice", listOf(file("Wav.kt")))

        assertEquals(
            listOf("src/", "  voice/", "    Wav.kt", "  main.kt", "README.md"),
            tree.drawn(),
        )
        // Paths stay relative and never lead with a slash: the daemon takes
        // exactly these.
        assertEquals(
            listOf("src", "src/voice", "src/voice/Wav.kt", "src/main.kt", "README.md"),
            tree.paths(),
        )
    }

    @Test
    fun closing_a_directory_remembers_what_was_open_inside_it() {
        val opened = FileTree()
            .withChildren("", listOf(dir("src")))
            .toggled("src")
            .withChildren("src", listOf(dir("voice")))
            .toggled("src/voice")
            .withChildren("src/voice", listOf(file("Wav.kt")))

        val closed = opened.toggled("src")
        assertEquals(listOf("src/"), closed.drawn())

        // Reopening shows it as it was left, not collapsed to one level, and
        // costs no round trip.
        val reopened = closed.toggled("src")
        assertEquals(listOf("src/", "  voice/", "    Wav.kt"), reopened.drawn())
        assertTrue(reopened.isLoaded("src/voice"))
    }

    @Test
    fun a_directory_being_listed_says_so_and_stops_when_it_lands() {
        val tree = FileTree()
            .withChildren("", listOf(dir("src")))
            .toggled("src")
            .loading("src")

        assertTrue(tree.rows().single { it.path == "src" }.loading)
        assertFalse(tree.isLoaded("src"))

        val landed = tree.withChildren("src", listOf(file("main.kt")))
        assertFalse(landed.rows().first().loading)
        assertEquals(listOf("src/", "  main.kt"), landed.drawn())
    }

    @Test
    fun a_listing_that_failed_leaves_the_directory_open_and_empty() {
        val tree = FileTree()
            .withChildren("", listOf(dir("secret")))
            .toggled("secret")
            .loading("secret")
            .failed("secret")

        // Open, not spinning, and contributing nothing -- rather than a row
        // that spins forever on a directory that cannot be read.
        val row = tree.rows().single()
        assertTrue(row.expanded)
        assertFalse(row.loading)
        assertEquals(1, tree.rows().size)
    }

    @Test
    fun refreshing_forgets_listings_but_not_what_was_open() {
        val tree = FileTree()
            .withChildren("", listOf(dir("src")))
            .toggled("src")
            .withChildren("src", listOf(file("gone.kt")))

        val stale = tree.invalidated()
        // The agent edits files while this is on screen, so a refresh has to
        // re-fetch rather than redraw a directory that may no longer match.
        assertFalse(stale.isLoaded("src"))
        assertTrue(stale.isExpanded("src"))
        assertEquals(emptyList(), stale.drawn())
    }

    @Test
    fun revealing_a_file_opens_every_directory_above_it() {
        val tree = FileTree().revealing("src/voice/Wav.kt")

        assertTrue(tree.isExpanded("src"))
        assertTrue(tree.isExpanded("src/voice"))
        // The file itself is not a directory to open.
        assertFalse(tree.isExpanded("src/voice/Wav.kt"))
    }

    @Test
    fun revealing_a_file_at_the_root_opens_nothing() {
        assertEquals(FileTree(), FileTree().revealing("README.md"))
    }
}
