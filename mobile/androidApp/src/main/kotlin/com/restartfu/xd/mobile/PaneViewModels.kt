package com.restartfu.xd.mobile

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.restartfu.xd.protocol.FileEntryReply
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
    val path: String = "",
    val entries: List<FileEntryReply> = emptyList(),
    val preview: String? = null,
    val previewPath: String = "",
    val loading: Boolean = false,
    val error: String? = null,
)

/**
 * A directory listing, or one previewed file.
 *
 * Paths stay relative to the chat's working directory because the daemon
 * refuses anything else; going up means dropping the last segment.
 */
class FilesViewModel(
    private val session: ChatSession,
) : ViewModel() {
    private val _state = MutableStateFlow(FilesPane())
    val state: StateFlow<FilesPane> = _state.asStateFlow()

    init {
        open(null)
    }

    fun open(path: String?) {
        _state.value = _state.value.copy(loading = true, error = null, preview = null)
        viewModelScope.launch {
            try {
                val entries = session.listDirectory(path)
                _state.value = FilesPane(path = path.orEmpty(), entries = entries)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _state.value = _state.value.copy(
                    loading = false,
                    error = error.message ?: "Could not list that directory",
                )
            }
        }
    }

    fun enter(entry: FileEntryReply) {
        val next = join(_state.value.path, entry.name)
        if (entry.directory) open(next) else preview(next)
    }

    fun up() {
        val current = _state.value
        if (current.preview != null) {
            open(current.path.takeIf { it.isNotEmpty() })
            return
        }
        if (current.path.isEmpty()) return
        open(current.path.substringBeforeLast('/', "").takeIf { it.isNotEmpty() })
    }

    fun refresh() {
        val current = _state.value
        if (current.preview != null) preview(current.previewPath) else open(current.path.takeIf { it.isNotEmpty() })
    }

    private fun preview(path: String) {
        _state.value = _state.value.copy(loading = true, error = null)
        viewModelScope.launch {
            try {
                val content = session.readFile(path)
                _state.value = _state.value.copy(
                    preview = content,
                    previewPath = path,
                    loading = false,
                )
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

    private fun join(base: String, name: String): String =
        if (base.isEmpty()) name else "$base/$name"

    class Factory(private val session: ChatSession) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T =
            FilesViewModel(session) as T
    }
}
