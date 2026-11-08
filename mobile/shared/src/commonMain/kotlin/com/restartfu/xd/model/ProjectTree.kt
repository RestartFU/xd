package com.restartfu.xd.model

/** A folder of projects, as the Projects list shows it. */
public data class MinimalFolder(
    val id: String,
    val name: String,
    /** The GitHub organization or user whose avatar it shows. */
    val owner: String?,
    val parentId: String?,
    /** The projects and folders directly inside it. */
    val items: Int,
)

/** One row of the Projects list, indented by how many folders it sits inside. */
public sealed interface ProjectTreeRow {
    public val id: String
    public val depth: Int

    public data class FolderRow(
        val folder: MinimalFolder,
        override val depth: Int,
        val open: Boolean,
    ) : ProjectTreeRow {
        override val id: String get() = folder.id
    }

    public data class ProjectRow(
        val project: MinimalProject,
        override val depth: Int,
    ) : ProjectTreeRow {
        override val id: String get() = project.id
    }
}

/**
 * The Projects list in the desktop sidebar's order: each folder followed by
 * what it holds, siblings in the host's order. A folder whose parent is not
 * listed sits at the top. Nothing inside a [collapsed] folder is listed.
 */
public fun TreeSnapshot.projectTree(collapsed: Set<String> = emptySet()): List<ProjectTreeRow> {
    val projects = minimalProjects().associateBy(MinimalProject::id)
    val listed = folders.mapTo(mutableSetOf(), Folder::id)
    val children = mutableMapOf<String, MutableList<Folder>>()
    val roots = mutableListOf<Folder>()
    for (folder in folders) {
        val parent = folder.parentId
        if (parent != null && parent != folder.id && parent in listed) {
            children.getOrPut(parent) { mutableListOf() } += folder
        } else {
            roots += folder
        }
    }
    val rows = mutableListOf<ProjectTreeRow>()
    val placed = mutableSetOf<String>()
    fun visit(start: Folder) {
        // Depth first without recursion, so a deep tree cannot overflow the stack.
        val stack = ArrayDeque(listOf(Triple(start, 0, false)))
        while (stack.isNotEmpty()) {
            val (folder, depth, hidden) = stack.removeLast()
            if (!placed.add(folder.id)) continue
            val open = folder.id !in collapsed
            if (!hidden) {
                rows += if (folder.group) {
                    ProjectTreeRow.FolderRow(
                        folder = MinimalFolder(
                            id = folder.id,
                            name = folder.name,
                            owner = folder.owner,
                            parentId = folder.parentId,
                            items = children[folder.id]?.size ?: 0,
                        ),
                        depth = depth,
                        open = open,
                    )
                } else {
                    ProjectTreeRow.ProjectRow(checkNotNull(projects[folder.id]), depth)
                }
            }
            // Only a folder of projects closes; what sits in a project always shows.
            val hideChildren = hidden || (folder.group && !open)
            children[folder.id]?.asReversed()?.forEach { stack.addLast(Triple(it, depth + 1, hideChildren)) }
        }
    }
    roots.forEach(::visit)
    // A parent loop is not reachable from the top; list it rather than lose it.
    folders.forEach(::visit)
    return rows
}

/** A folder that something can be moved into, indented as the list shows it. */
public data class FolderDestination(
    val folder: MinimalFolder,
    val depth: Int,
)

/**
 * The folders [movingId] can be moved into: every folder of projects except
 * itself and the ones inside it, which the host refuses.
 */
public fun TreeSnapshot.moveDestinations(movingId: String): List<FolderDestination> =
    projectTree().mapNotNull { row ->
        (row as? ProjectTreeRow.FolderRow)
            ?.takeUnless { isInside(it.folder.id, movingId) }
            ?.let { FolderDestination(it.folder, it.depth) }
    }

/** Whether [id] is [ancestorId] or sits somewhere inside it. */
public fun TreeSnapshot.isInside(id: String, ancestorId: String): Boolean {
    val parents = folders.associate { it.id to it.parentId }
    var current: String? = id
    val seen = mutableSetOf<String>()
    while (current != null && seen.add(current)) {
        if (current == ancestorId) return true
        current = parents[current]
    }
    return false
}

/** The folders [id] sits inside, outermost first. */
public fun TreeSnapshot.ancestors(id: String): List<String> {
    val parents = folders.associate { it.id to it.parentId }
    val found = mutableListOf<String>()
    var current = parents[id]
    while (current != null && current != id && current !in found) {
        found += current
        current = parents[current]
    }
    return found.asReversed()
}

/**
 * The GitHub login a typed organization names: `acme`, `@acme`, or its
 * GitHub page such as `github.com/acme`. Null for anything else, including a
 * repository's page, as the desktop accepts.
 */
public fun organizationLogin(typed: String): String? {
    val trimmed = typed.trim().trimEnd('/')
    val path = trimmed.substringAfter("://", trimmed).removePrefix("www.")
    val login = if ('/' in path) {
        val host = path.substringBefore('/')
        if (!host.equals("github.com", ignoreCase = true)) return null
        val rest = path.substringAfter('/')
        if ('/' in rest) return null
        rest
    } else {
        path.removePrefix("@")
    }
    val valid = login.isNotEmpty() &&
        login.length <= 39 &&
        !login.startsWith('-') &&
        !login.endsWith('-') &&
        login.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '-' }
    return login.takeIf { valid }
}
