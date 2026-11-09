package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ProjectTreeTest {
    private val snapshot = TreeSnapshot(
        folders = listOf(
            Folder("notes", "notes", null),
            Folder("web", "web", "acme"),
            Folder("acme", "acme", null, group = true, owner = "acme"),
            Folder("clients", "Clients", "acme", group = true),
            Folder("site", "site", "clients", owner = "acme"),
        ),
        folderGroups = true,
    )

    private fun List<ProjectTreeRow>.shape() = map { row ->
        "  ".repeat(row.depth) + when (row) {
            is ProjectTreeRow.FolderRow -> "[${row.folder.name}${if (row.open) "" else " closed"}]"
            is ProjectTreeRow.ProjectRow -> row.project.name
        }
    }

    @Test
    fun foldersListWhatTheyHoldBeneathThemInTheHostsOrder() {
        // A child listed before its parent still lands beneath it.
        assertEquals(
            listOf("notes", "[acme]", "  web", "  [Clients]", "    site"),
            snapshot.projectTree().shape(),
        )
        val acme = snapshot.projectTree().filterIsInstance<ProjectTreeRow.FolderRow>().first()
        assertEquals(2, acme.folder.items)
        assertEquals("acme", acme.folder.owner)
    }

    @Test
    fun aClosedFolderHidesEverythingInsideIt() {
        assertEquals(listOf("notes", "[acme closed]"), snapshot.projectTree(setOf("acme")).shape())
        assertEquals(
            listOf("notes", "[acme]", "  web", "  [Clients closed]"),
            snapshot.projectTree(setOf("clients")).shape(),
        )
        // Only a folder of projects closes.
        assertEquals(snapshot.projectTree().shape(), snapshot.projectTree(setOf("notes", "web")).shape())
    }

    @Test
    fun projectsLeaveFoldersOut() {
        assertEquals(listOf("notes", "web", "site"), snapshot.minimalProjects().map { it.name })
        assertEquals("acme", snapshot.minimalProjects().last().owner)
        assertEquals("clients", snapshot.minimalProjects().last().parentId)
    }

    @Test
    fun aParentLoopIsStillListed() {
        val looped = TreeSnapshot(
            folders = listOf(
                Folder("a", "a", "b", group = true),
                Folder("b", "b", "a", group = true),
                Folder("top", "top", "missing"),
            ),
        )
        assertEquals(listOf("top", "[a]", "  [b]"), looped.projectTree().shape())
    }

    @Test
    fun aFolderHoldsTheProjectsNestedBeneathItAndNothingAfter() {
        assertEquals(listOf("web", "site"), snapshot.projectsInside("acme"))
        assertEquals(listOf("site"), snapshot.projectsInside("clients"))
        // A project is not a folder of projects, and an unknown id holds nothing.
        assertEquals(emptyList(), snapshot.projectsInside("notes"))
        assertEquals(emptyList(), snapshot.projectsInside("missing"))
    }

    @Test
    fun somethingMovesOnlyIntoFoldersOutsideItself() {
        assertEquals(listOf("acme", "clients"), snapshot.moveDestinations("web").map { it.folder.id })
        assertEquals(listOf(0, 1), snapshot.moveDestinations("web").map { it.depth })
        assertEquals(emptyList(), snapshot.moveDestinations("acme").map { it.folder.id })
        assertEquals(listOf("acme"), snapshot.moveDestinations("clients").map { it.folder.id })
        assertTrue(snapshot.isInside("site", "acme"))
        assertFalse(snapshot.isInside("notes", "acme"))
        assertEquals(listOf("acme", "clients"), snapshot.ancestors("site"))
    }

    @Test
    fun foldersCanBeMadeWhenTheHostKeepsThem() {
        assertTrue(snapshot.canCreateFolders)
        assertFalse(TreeSnapshot(folders = listOf(Folder("p", "p", null))).canCreateFolders)
        // A v0.2.4 host keeps folders without saying so.
        assertTrue(TreeSnapshot(folders = listOf(Folder("g", "g", null, group = true))).canCreateFolders)
    }

    @Test
    fun organizationNamesComeFromALoginOrItsGitHubPage() {
        for ((typed, login) in listOf(
            "acme" to "acme",
            "  @acme-inc " to "acme-inc",
            "github.com/acme" to "acme",
            "https://www.github.com/Acme/" to "Acme",
            "https://github.com/acme/web" to null,
            "https://gitlab.com/acme" to null,
            "acme corp" to null,
            "-acme" to null,
            "" to null,
            "../acme" to null,
        )) {
            assertEquals(login, organizationLogin(typed), typed)
        }
    }
}
