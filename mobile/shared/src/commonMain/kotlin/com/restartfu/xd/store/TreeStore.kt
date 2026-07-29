package com.restartfu.xd.store

import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.Folder
import com.restartfu.xd.model.TreeSnapshot
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.TreeReply
import com.restartfu.xd.protocol.decodeReply
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

internal class TreeStore(
    private val actor: ConnectionActor,
) {
    private val refreshMutex = Mutex()
    private val stateMutex = Mutex()
    private val _state = MutableStateFlow(TreeSnapshot())
    private var lifecycleVersion = 0L
    private val workingEvents = mutableMapOf<String, WorkingEvent>()

    val state: StateFlow<TreeSnapshot> = _state.asStateFlow()

    suspend fun clear() {
        stateMutex.withLock {
            lifecycleVersion += 1
            workingEvents.clear()
            _state.value = TreeSnapshot()
        }
    }

    suspend fun setChatWorking(
        chatId: String,
        working: Boolean,
    ) {
        stateMutex.withLock {
            lifecycleVersion += 1
            workingEvents[chatId] = WorkingEvent(lifecycleVersion, working)
            _state.value = _state.value.copy(
                chats = _state.value.chats.map { chat ->
                    if (chat.id == chatId) chat.copy(working = working) else chat
                },
            )
        }
    }

    suspend fun refresh() {
        refreshMutex.withLock {
            val versionBefore = stateMutex.withLock {
                _state.value = _state.value.copy(loading = true, error = null)
                lifecycleVersion
            }
            try {
                val reply = actor.call(Ops.tree()).decodeReply<TreeReply>()
                stateMutex.withLock {
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
                                working = workingEvents[it.id]
                                    ?.takeIf { event -> event.version > versionBefore }
                                    ?.working
                                    ?: it.working,
                            )
                        },
                    )
                    workingEvents.entries.removeAll { it.value.version <= versionBefore }
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                stateMutex.withLock {
                    _state.value = _state.value.copy(
                        loading = false,
                        error = error.message ?: "Could not load the workspace tree",
                    )
                }
            }
        }
    }

    private data class WorkingEvent(
        val version: Long,
        val working: Boolean,
    )
}
