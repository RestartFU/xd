package com.restartfu.xd.files

import com.restartfu.xd.protocol.FileEntryReply

/** One line of the drawn tree: an entry, and how deep it sits. */
public data class TreeRow(
    /** Relative to the chat's working directory, which is what the host takes. */
    public val path: String,
    public val name: String,
    public val directory: Boolean,
    public val depth: Int,
    /** Open, so its children are the rows below this one. */
    public val expanded: Boolean,
    /** Its listing is in flight. */
    public val loading: Boolean,
)

/**
 * A directory tree that folds, over a host that only lists one directory at a
 * time.
 *
 * The listing call takes a path and answers with that path's entries, so the
 * tree is assembled here: children arrive per directory and are kept, and what
 * is drawn is the walk from the root through whichever directories are open.
 * Folding is then free, and reopening a directory costs nothing.
 *
 * Immutable, because it is read on the main thread while listings land from
 * another: every change returns a new tree rather than being seen half-applied.
 *
 * Paths are relative and never begin with a slash -- the root is `""` -- since
 * that is the only shape the host accepts.
 */
public data class FileTree(
    private val children: Map<String, List<FileEntryReply>> = emptyMap(),
    private val expanded: Set<String> = emptySet(),
    private val loading: Set<String> = emptySet(),
) {
    /**
     * The tree flattened for a list, parents before their children.
     *
     * A directory whose listing has not arrived contributes no rows of its own,
     * so an open-but-empty directory and an open-but-still-loading one look the
     * same until it lands -- which is what [TreeRow.loading] is for.
     */
    public fun rows(): List<TreeRow> = buildList { walk("", 0, this) }

    /** Whether [path]'s listing has been fetched. */
    public fun isLoaded(path: String): Boolean = children.containsKey(path)

    public fun isExpanded(path: String): Boolean = path in expanded

    /**
     * Opens or closes a directory.
     *
     * Closing keeps its children, and keeps which of them were open: a folder
     * reopened should look the way it was left, not collapsed to one level.
     */
    public fun toggled(path: String): FileTree = copy(
        expanded = if (path in expanded) expanded - path else expanded + path,
    )

    /** Marks a directory as being listed, which draws its spinner. */
    public fun loading(path: String): FileTree = copy(loading = loading + path)

    /** Takes a directory's entries, sorted the way a tree reads. */
    public fun withChildren(path: String, entries: List<FileEntryReply>): FileTree = copy(
        children = children + (path to entries.sortedWith(ORDER)),
        loading = loading - path,
    )

    /** Gives up on a listing, leaving the directory open and empty. */
    public fun failed(path: String): FileTree = copy(loading = loading - path)

    /**
     * Forgets every listing, keeping what was open.
     *
     * A refresh has to re-fetch rather than redraw: the agent edits files, and
     * a tree that trusted its cache would go on showing a directory that is no
     * longer there.
     */
    public fun invalidated(): FileTree = copy(children = emptyMap(), loading = emptySet())

    /**
     * Opens every directory on the way to [path], so a file can be revealed
     * without the reader clicking down to it.
     */
    public fun revealing(path: String): FileTree {
        val opened = expanded.toMutableSet()
        var at = 0
        while (true) {
            val slash = path.indexOf('/', at)
            if (slash < 0) break
            opened += path.substring(0, slash)
            at = slash + 1
        }
        return copy(expanded = opened)
    }

    private fun walk(path: String, depth: Int, into: MutableList<TreeRow>) {
        for (entry in children[path].orEmpty()) {
            val child = if (path.isEmpty()) entry.name else "$path/${entry.name}"
            val open = entry.directory && child in expanded
            into += TreeRow(
                path = child,
                name = entry.name,
                directory = entry.directory,
                depth = depth,
                expanded = open,
                loading = child in loading,
            )
            if (open) walk(child, depth + 1, into)
        }
    }

    private companion object {
        /** Directories first, then by name: the order every file tree uses. */
        val ORDER = compareBy<FileEntryReply>({ !it.directory }, { it.name.lowercase() })
    }
}
