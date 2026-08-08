package com.restartfu.xd.files

/** One file open in a tab. */
public data class OpenFile(
    /** Relative to the chat's working directory. */
    public val path: String,
    /** What the daemon last handed over, and what a write is checked against. */
    public val saved: String,
    /** What is on screen, which is [saved] until it is typed in. */
    public val text: String = saved,
    public val saving: Boolean = false,
    public val error: String? = null,
) {
    /** Whether there is anything to save. */
    public val dirty: Boolean get() = text != saved

    /** The tab's label: the file's own name, not the path it took to get here. */
    public val name: String get() = path.substringAfterLast('/')
}

/**
 * The files open beside the chat, and which one is in front.
 *
 * A tab strip rather than one preview at a time: reading code means holding two
 * files and a conversation at once, and a browser that replaces its contents
 * makes that a matter of remembering where things were.
 *
 * The tree is not in this list. It is always there and cannot be closed, so it
 * is [FileTab.Tree] -- one case in [active] rather than an entry that every
 * caller has to remember not to close.
 */
public data class OpenFiles(
    public val files: List<OpenFile> = emptyList(),
    public val active: FileTab = FileTab.Tree,
) {
    /** Whichever file is in front, or null when the tree is. */
    public val current: OpenFile?
        get() = (active as? FileTab.File)?.let { tab -> files.find { it.path == tab.path } }

    public val anyDirty: Boolean get() = files.any { it.dirty }

    /**
     * Brings a file to the front, opening it if it is not open yet.
     *
     * Opening the same file twice is the same tab: two tabs on one path would
     * be two sets of edits to reconcile, and no editor offers that.
     */
    public fun opened(path: String, saved: String): OpenFiles {
        val existing = files.any { it.path == path }
        return copy(
            files = if (existing) files else files + OpenFile(path = path, saved = saved),
            active = FileTab.File(path),
        )
    }

    /**
     * Closes a tab, landing on the one to its left -- or the tree, which is
     * always there to land on.
     */
    public fun closed(path: String): OpenFiles {
        val at = files.indexOfFirst { it.path == path }
        if (at < 0) return this
        val rest = files.filterIndexed { index, _ -> index != at }
        val next = when {
            active != FileTab.File(path) -> active
            rest.isEmpty() -> FileTab.Tree
            else -> FileTab.File(rest[(at - 1).coerceAtLeast(0)].path)
        }
        return copy(files = rest, active = next)
    }

    public fun showing(tab: FileTab): OpenFiles = copy(active = tab)

    /** Takes a keystroke. */
    public fun edited(path: String, text: String): OpenFiles =
        mapping(path) { it.copy(text = text, error = null) }

    public fun saving(path: String): OpenFiles = mapping(path) { it.copy(saving = true, error = null) }

    /**
     * Accepts a write.
     *
     * The saved text is what was *sent*, not what is on screen now: typing
     * carries on during the round trip, and taking the current text would mark
     * those later keystrokes as already written.
     */
    public fun saved(path: String, written: String): OpenFiles =
        mapping(path) { it.copy(saved = written, saving = false, error = null) }

    public fun failed(path: String, message: String): OpenFiles =
        mapping(path) { it.copy(saving = false, error = message) }

    private fun mapping(path: String, change: (OpenFile) -> OpenFile): OpenFiles =
        copy(files = files.map { if (it.path == path) change(it) else it })
}

/** What the pane is showing. */
public sealed interface FileTab {
    public data object Tree : FileTab

    public data class File(public val path: String) : FileTab
}
