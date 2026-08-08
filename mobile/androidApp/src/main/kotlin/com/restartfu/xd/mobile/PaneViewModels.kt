package com.restartfu.xd.mobile

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.restartfu.xd.files.FileTab
import com.restartfu.xd.files.FileTree
import com.restartfu.xd.files.OpenFiles
import com.restartfu.xd.store.ChatSession
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

public data class DiffPane(
    val branch: Boolean = false,
    val patch: String = "",
    val collapsedFiles: Set<String> = emptySet(),
    val loading: Boolean = false,
    val error: String? = null,
)

/**
 * The working or branch patch for a chat.
 *
 * `branch-all` needs the branch point, which is a separate `base` read, so
 * switching scope costs two calls rather than one.
 */
class DiffViewModel(
    private val session: ChatSession,
) : ViewModel() {
    private val _state = MutableStateFlow(DiffPane())
    val state: StateFlow<DiffPane> = _state.asStateFlow()

    init {
        refresh()
    }

    fun showBranch(branch: Boolean) {
        if (_state.value.branch == branch) return
        _state.value = _state.value.copy(branch = branch, patch = "")
        refresh()
    }

    fun toggleFile(path: String) {
        val collapsed = _state.value.collapsedFiles
        _state.value = _state.value.copy(
            collapsedFiles = if (path in collapsed) collapsed - path else collapsed + path,
        )
    }

    fun refresh() {
        val branch = _state.value.branch
        _state.value = _state.value.copy(loading = true, error = null)
        viewModelScope.launch {
            try {
                val patch = if (branch) {
                    session.diff("branch-all", session.diffBase())
                } else {
                    session.diff("working-all")
                }
                _state.value = _state.value.copy(patch = patch, loading = false)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _state.value = _state.value.copy(
                    loading = false,
                    error = error.message ?: "Could not read the diff",
                )
            }
        }
    }

    class Factory(private val session: ChatSession) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T =
            DiffViewModel(session) as T
    }
}

public data class FilesPane(
    val tree: FileTree = FileTree(),
    val open: OpenFiles = OpenFiles(),
    val loading: Boolean = false,
    val error: String? = null,
)

/**
 * The working directory as a tree, and the files opened out of it.
 *
 * The daemon lists one directory per call, so the tree is filled in as folders
 * are opened rather than walked up front -- a repository is far too big to send
 * whole, and most of it is never looked at.
 */
class FilesViewModel(
    private val session: ChatSession,
) : ViewModel() {
    private val _state = MutableStateFlow(FilesPane())
    val state: StateFlow<FilesPane> = _state.asStateFlow()

    init {
        list("")
    }

    /** Opens or closes a folder, fetching its entries the first time. */
    fun toggle(path: String) {
        val current = _state.value
        val opening = !current.tree.isExpanded(path)
        _state.value = current.copy(tree = current.tree.toggled(path), error = null)
        if (opening && !current.tree.isLoaded(path)) list(path)
    }

    /** Opens a file in a tab, or brings it forward if it is already open. */
    fun open(path: String) {
        val already = _state.value.open.files.any { it.path == path }
        if (already) {
            show(FileTab.File(path))
            return
        }
        _state.value = _state.value.copy(loading = true, error = null)
        viewModelScope.launch {
            try {
                val content = session.readFile(path)
                _state.value = _state.value.let {
                    it.copy(open = it.open.opened(path, content), loading = false)
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                // A binary or oversized file is a normal answer here, not a
                // failure of the pane.
                _state.value = _state.value.copy(
                    loading = false,
                    error = error.message ?: "Could not read that file",
                )
            }
        }
    }

    fun show(tab: FileTab) {
        _state.value = _state.value.let { it.copy(open = it.open.showing(tab)) }
    }

    fun close(path: String) {
        _state.value = _state.value.let { it.copy(open = it.open.closed(path)) }
    }

    fun edit(path: String, text: String) {
        _state.value = _state.value.let { it.copy(open = it.open.edited(path, text)) }
    }

    /**
     * Writes the open file back.
     *
     * The text is captured before the round trip and reported as what landed,
     * so anything typed while it was in flight stays unsaved rather than being
     * marked written.
     */
    fun save(path: String) {
        val file = _state.value.open.files.find { it.path == path } ?: return
        if (file.saving || !file.dirty) return
        val sending = file.text
        val against = file.saved
        _state.value = _state.value.let { it.copy(open = it.open.saving(path)) }
        viewModelScope.launch {
            try {
                session.writeFile(path, against, sending)
                _state.value = _state.value.let { it.copy(open = it.open.saved(path, sending)) }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _state.value = _state.value.let {
                    it.copy(
                        open = it.open.failed(
                            path,
                            error.message ?: "Could not save that file",
                        ),
                    )
                }
            }
        }
    }

    /**
     * Re-reads the tree, keeping every folder open.
     *
     * Open files are left alone: the agent edits the same tree, and dropping
     * what someone has typed because a sibling directory changed would be a
     * poor trade.
     */
    fun refresh() {
        val current = _state.value
        _state.value = current.copy(tree = current.tree.invalidated(), error = null)
        list("")
        for (row in current.tree.rows()) {
            if (row.directory && row.expanded) list(row.path)
        }
    }

    private fun list(path: String) {
        _state.value = _state.value.let { it.copy(tree = it.tree.loading(path)) }
        viewModelScope.launch {
            try {
                val entries = session.listDirectory(path.takeIf { it.isNotEmpty() })
                _state.value = _state.value.let {
                    it.copy(tree = it.tree.withChildren(path, entries))
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _state.value = _state.value.let {
                    it.copy(
                        tree = it.tree.failed(path),
                        error = error.message ?: "Could not list that directory",
                    )
                }
            }
        }
    }

    class Factory(private val session: ChatSession) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T =
            FilesViewModel(session) as T
    }
}
