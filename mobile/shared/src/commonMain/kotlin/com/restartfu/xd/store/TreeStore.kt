package com.restartfu.xd.store

import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.Folder
import com.restartfu.xd.model.TreeSnapshot
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.TreeReply
import com.restartfu.xd.protocol.decodeReply
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

internal class TreeStore(
    private val actor: ConnectionActor,
) {
    private val refreshMutex = Mutex()
    private val _state = MutableStateFlow(TreeSnapshot())

    val state: StateFlow<TreeSnapshot> = _state.asStateFlow()

    suspend fun refresh() {
        refreshMutex.withLock {
            _state.value = _state.value.copy(loading = true, error = null)
            try {
                val reply = actor.call(Ops.tree()).decodeReply<TreeReply>()
                _state.value = TreeSnapshot(
                    folders = reply.folders.map {
                        Folder(id = it.id, name = it.name, parentId = it.parent)
                    },
                    chats = reply.chats.map {
                        ChatSummary(
                            id = it.id,
                            folderId = it.folder,
                            title = it.title,
                            backend = it.backend,
                            working = it.working,
                        )
                    },
                )
            } catch (error: Throwable) {
                _state.value = _state.value.copy(
                    loading = false,
                    error = error.message ?: "Could not load the workspace tree",
                )
            }
        }
    }
}
