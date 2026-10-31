package com.restartfu.xd.store

import com.restartfu.xd.model.ChatSummary
import com.restartfu.xd.model.Folder
import com.restartfu.xd.model.TreeSnapshot
import com.restartfu.xd.model.WorkspaceSession
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
    private var clearVersion = 0L
    private var snapshotSequence = 0L
    private val workingEvents = mutableMapOf<String, WorkingEvent>()
    private val terminalWorkingEvents = mutableMapOf<String, WorkingEvent>()

    val state: StateFlow<TreeSnapshot> = _state.asStateFlow()

    fun terminalChatIds(owner: String): Set<String> = terminalChatIds(_state.value, owner)

    fun terminalOwner(owner: String): String {
        val snapshot = _state.value
        return snapshot.sessions.firstOrNull {
            it.terminalChatId == owner || it.id == owner || owner in it.chatIds
        }?.terminalChatId ?: owner
    }

    fun terminalChatIds(snapshot: TreeSnapshot, owner: String): Set<String> {
        snapshot.sessions.firstOrNull {
            it.terminalChatId == owner || it.id == owner || owner in it.chatIds
        }?.let { return it.chatIds.toSet() }
        val sessionId = if (owner.startsWith("session:")) owner.removePrefix("session:") else {
            snapshot.chats.firstOrNull { it.id == owner }?.sessionId ?: return emptySet()
        }
        return snapshot.chats.filter { it.sessionId == sessionId }.map { it.id }.toSet()
    }

    suspend fun clear() {
        stateMutex.withLock {
            lifecycleVersion += 1
            clearVersion = lifecycleVersion
            workingEvents.clear()
            terminalWorkingEvents.clear()
            _state.value = TreeSnapshot()
        }
    }

    suspend fun setChatWorking(
        chatId: String,
        working: Boolean,
        sequence: Long,
    ) {
        stateMutex.withLock {
            if (sequence <= snapshotSequence) return
            if ((workingEvents[chatId]?.sequence ?: Long.MIN_VALUE) >= sequence) return
            lifecycleVersion += 1
            workingEvents[chatId] = WorkingEvent(
                sequence = sequence,
                working = working,
            )
            _state.value = _state.value.copy(
                chats = _state.value.chats.map { chat ->
                    if (chat.id == chatId) chat.copy(working = working) else chat
                },
            )
        }
    }

    suspend fun setChatTerminalWorking(
        chatId: String,
        working: Boolean,
        sequence: Long,
    ) {
        stateMutex.withLock {
            if (sequence <= snapshotSequence) return
            if ((terminalWorkingEvents[chatId]?.sequence ?: Long.MIN_VALUE) >= sequence) return
            lifecycleVersion += 1
            terminalWorkingEvents[chatId] = WorkingEvent(sequence, working)
            val members = terminalChatIds(_state.value, chatId)
            _state.value = _state.value.copy(
                chats = _state.value.chats.map { chat ->
                    if (chat.id in members) chat.copy(terminalWorking = working) else chat
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
                val result = actor.callSequenced(Ops.tree())
                val reply = actor.decodeReply(result.value) {
                    it.decodeReply<TreeReply>()
                }
                stateMutex.withLock {
                    if (clearVersion > versionBefore) return@withLock
                    snapshotSequence = maxOf(snapshotSequence, result.sequence)
                    val authoritativeSequence = snapshotSequence
                    _state.value = TreeSnapshot(
                        folders = reply.folders.map {
                            Folder(id = it.id, name = it.name, parentId = it.parent)
                        },
                        sessions = reply.sessions.map {
                            WorkspaceSession(it.id, it.folder, it.title, it.chats,
                                it.branch, it.working, it.terminalChat)
                        },
                        chats = reply.chats.map {
                            ChatSummary(
                                id = it.id,
                                folderId = it.folder,
                                title = it.title,
                                backend = it.backend,
                                sessionMode = it.sessionMode,
                                sessionId = it.session ?: it.id,
                                branch = it.branch,
                                working = workingEvents[it.id]
                                    ?.takeIf { event ->
                                        event.sequence > authoritativeSequence
                                    }
                                    ?.working
                                    ?: it.working,
                                terminalWorking = terminalWorkingEvents.entries
                                    .filter { (owner, _) ->
                                        owner == it.id || owner == "session:${it.session ?: it.id}" ||
                                            reply.sessions.any { session ->
                                                it.id in session.chats && (owner == session.terminalChat || owner == session.id || owner in session.chats)
                                            }
                                    }
                                    .maxByOrNull { (_, event) -> event.sequence }?.value
                                    ?.takeIf { event ->
                                        event.sequence > authoritativeSequence
                                    }
                                    ?.working
                                    ?: it.terminalWorking,
                            )
                        },
                    )
                    workingEvents.entries.removeAll {
                        it.value.sequence <= authoritativeSequence
                    }
                    terminalWorkingEvents.entries.removeAll {
                        it.value.sequence <= authoritativeSequence
                    }
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
        val sequence: Long,
        val working: Boolean,
    )
}
