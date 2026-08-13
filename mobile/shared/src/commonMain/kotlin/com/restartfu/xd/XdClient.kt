package com.restartfu.xd

import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.model.TreeSnapshot
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.net.Link
import com.restartfu.xd.net.PairResult
import com.restartfu.xd.net.PlatformSocketFactory
import com.restartfu.xd.protocol.HostUpdateReply
import com.restartfu.xd.protocol.DirectoryListReply
import com.restartfu.xd.protocol.DoneReply
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.RemoteProtocolException
import com.restartfu.xd.protocol.ShortcutsReply
import com.restartfu.xd.protocol.WorkflowStatusReply
import com.restartfu.xd.protocol.decodeReply
import com.restartfu.xd.store.ChatSession
import com.restartfu.xd.store.ChatSessionCore
import com.restartfu.xd.store.TreeStore
import com.restartfu.xd.time.currentEpochMillis
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.coroutines.launch
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.JsonPrimitive
import com.restartfu.xd.terminal.TerminalEvent
import com.restartfu.xd.terminal.TerminalWire

private fun validatedDeviceName(name: String): String {
    require(name.isNotBlank()) { "Device name must not be blank" }
    return name
}

public class XdClient(
    socketFactory: PlatformSocketFactory,
    credentials: CredentialStore,
    private val scope: CoroutineScope,
    deviceName: String = automaticDeviceName(),
) {
    private val actor = ConnectionActor(
        socketFactory,
        credentials,
        scope,
        validatedDeviceName(deviceName),
    )
    private val treeStore = TreeStore(actor)
    private val treeRefreshRequests = Channel<Unit>(Channel.CONFLATED)
    private val sessions = MutableStateFlow<Map<String, SessionEntry>>(emptyMap())

    public val link: StateFlow<Link> = actor.link
    public val hasCredentials: StateFlow<Boolean> = actor.hasCredentials
    public val credentialsReady: StateFlow<Boolean> = actor.credentialsReady
    public val tree: StateFlow<TreeSnapshot> = treeStore.state

    /**
     * Terminal traffic, which the chat stores do not model: a pty's output is
     * not transcript state, so it is delivered decoded and on its own.
     */
    public val terminalEvents: Flow<TerminalEvent> =
        actor.events.mapNotNull { TerminalWire.event(it.value) }

    init {
        scope.launch {
            for (ignored in treeRefreshRequests) treeStore.refresh()
        }
        scope.launch {
            actor.link.collect { value ->
                if (value is Link.Up) {
                    requestTreeRefresh()
                    sessions.value.values.forEach { entry ->
                        entry.core.requestReload()
                    }
                }
            }
        }
        scope.launch {
            actor.events.collect { event ->
                val eventName =
                    (event.value["event"] as? JsonPrimitive)?.contentOrNull
                if (eventName == "tree") {
                    requestTreeRefresh()
                    sessions.value.values.forEach { entry ->
                        entry.core.requestReload()
                    }
                }
                val chatId = (event.value["chat"] as? JsonPrimitive)?.contentOrNull
                if (chatId != null && eventName in TURN_LIFECYCLE_EVENTS) {
                    treeStore.setChatWorking(
                        chatId = chatId,
                        working = eventName == "turn-started",
                        sequence = event.sequence,
                    )
                }
                if (chatId != null && eventName == "terminal-activity") {
                    val working = (event.value["terminal_working"] as? JsonPrimitive)
                        ?.booleanOrNull
                    if (working != null) {
                        treeStore.setChatTerminalWorking(chatId, working, event.sequence)
                    }
                }
                sessions.value.values.forEach { entry ->
                    // Preserve wire order: text/tool transitions are not
                    // commutative, so separate child jobs are unsafe here.
                    entry.core.onEvent(event)
                }
            }
        }
    }

    public fun poke() {
        actor.poke()
    }

    public fun goBackground() {
        actor.goBackground()
    }

    public suspend fun pair(
        host: String,
        port: Int,
        code: String,
    ): PairResult = actor.pair(host, port, code)

    @Deprecated(
        "The device name comes from the connecting platform; the supplied name is ignored.",
        ReplaceWith("pair(host, port, code)"),
    )
    public suspend fun pair(
        host: String,
        port: Int,
        code: String,
        _deviceName: String,
    ): PairResult = pair(host, port, code)

    public suspend fun forget() {
        actor.forget()
        treeStore.clear()
        takeSessions().values.forEach { it.core.invalidate() }
    }

    public fun openChat(chatId: String): ChatSession {
        require(chatId.isNotBlank()) { "Chat id must not be blank" }
        var chosen: ChatSessionCore
        while (true) {
            val before = sessions.value
            val existing = before[chatId]
            val created = if (existing == null) {
                ChatSessionCore(
                    chatId = chatId,
                    actor = actor,
                    scope = scope,
                    nowMillis = ::nowMillis,
                )
            } else {
                null
            }
            chosen = existing?.core ?: checkNotNull(created)
            val after = before + (
                chatId to SessionEntry(
                    core = chosen,
                    references = (existing?.references ?: 0) + 1,
                )
            )
            if (sessions.compareAndSet(before, after)) break
            created?.shutdown()
        }
        if (chosen.state.value.title.isEmpty() && actor.link.value is Link.Up) {
            chosen.requestReload()
        }
        return ChatSession(chosen) { releaseChat(chatId, chosen) }
    }

    /**
     * Asks about, or performs, an update of the host this client is paired
     * with. Connection-level rather than chat-level: it is the machine being
     * updated, not a conversation.
     */
    public suspend fun hostUpdate(action: String = "status"): HostUpdateReply {
        val value = actor.call(Ops.hostUpdate(action))
        return actor.decodeReply(value) { it.decodeReply<HostUpdateReply>() }
    }

    public suspend fun workflowStatus(marker: String): WorkflowStatusReply {
        val value = actor.call(Ops.workflowStatus(marker))
        return actor.decodeReply(value) { value ->
            value.decodeReply<WorkflowStatusReply>()
        }
    }

    public suspend fun shortcuts(folderId: String? = null): ShortcutsReply {
        val value = actor.call(Ops.shortcuts(folderId))
        return actor.decodeReply(value) { it.decodeReply<ShortcutsReply>() }
    }

    public suspend fun setShortcuts(
        folderId: String? = null,
        prompts: List<String>,
    ): ShortcutsReply {
        val value = actor.call(Ops.setShortcuts(folderId, prompts))
        return actor.decodeReply(value) { it.decodeReply<ShortcutsReply>() }
    }

    /**
     * Deletes a chat. Tree-level rather than session-level: the chat it would
     * belong to is exactly what stops existing.
     */
    public suspend fun deleteChat(chatId: String) {
        require(chatId.isNotBlank()) { "Chat id must not be blank" }
        actor.call(Ops.deleteChat(chatId))
        // The host broadcasts a tree event, but refreshing here means the
        // row is gone by the time the confirmation dismisses.
        requestTreeRefresh()
    }

    /**
     * Renames a chat. Tree-level alongside [deleteChat]: a title is how the
     * tree names a chat, not something the conversation holds.
     */
    public suspend fun renameChat(chatId: String, title: String) {
        require(chatId.isNotBlank()) { "Chat id must not be blank" }
        actor.call(Ops.renameChat(chatId, title))
        requestTreeRefresh()
        // Renaming broadcasts `tree` and nothing else, so a chat already open
        // would keep showing its old title in the header until something else
        // reloaded it.
        sessions.value[chatId]?.core?.requestReload()
    }

    public suspend fun createChat(
        folderId: String,
        title: String?,
        backend: String? = null,
    ): String {
        val value = actor.call(Ops.newChat(folderId, title, backend))
        return actor.decodeReply(value) {
            val reply = it.decodeReply<DoneReply>()
            reply.id?.takeIf(String::isNotBlank)
                ?: throw RemoteProtocolException("New chat reply has no id")
        }
    }

    public suspend fun createFolder(
        name: String,
        parentId: String? = null,
        repository: String? = null,
        repositoryUrl: String? = null,
    ): String {
        val value = actor.call(
            Ops.newFolder(name, parentId, repository, repositoryUrl),
        )
        val id = actor.decodeReply(value) {
            val reply = it.decodeReply<DoneReply>()
            reply.id?.takeIf(String::isNotBlank)
                ?: throw RemoteProtocolException("New folder reply has no id")
        }
        // Do not wait for the host's tree broadcast: show the new workspace as
        // soon as the creation dialog closes.
        requestTreeRefresh()
        return id
    }

    public suspend fun listDirectories(path: String? = null): DirectoryListReply {
        val value = actor.call(Ops.listDirectories(path))
        return actor.decodeReply(value) {
            it.decodeReply<DirectoryListReply>()
        }
    }

    public suspend fun moveFolder(
        folderId: String,
        parentId: String? = null,
    ) {
        require(folderId.isNotBlank()) { "Folder id must not be blank" }
        actor.call(Ops.moveFolder(folderId, parentId))
        requestTreeRefresh()
        // Moving a folder can change inherited settings for every chat below it.
        sessions.value.values.forEach { entry -> entry.core.requestReload() }
    }

    public suspend fun moveChat(
        chatId: String,
        folderId: String,
    ) {
        require(chatId.isNotBlank()) { "Chat id must not be blank" }
        require(folderId.isNotBlank()) { "Folder id must not be blank" }
        actor.call(Ops.moveChat(chatId, folderId))
        requestTreeRefresh()
        // A chat inherits backend, model, and workdir from its folder.
        sessions.value[chatId]?.core?.requestReload()
    }

    private fun takeSessions(): Map<String, SessionEntry> {
        while (true) {
            val before = sessions.value
            if (before.isEmpty()) return emptyMap()
            if (sessions.compareAndSet(before, emptyMap())) return before
        }
    }

    private fun releaseChat(
        chatId: String,
        expected: ChatSessionCore,
    ) {
        while (true) {
            val before = sessions.value
            val existing = before[chatId] ?: return
            if (existing.core !== expected) return
            val after = if (existing.references <= 1) {
                before - chatId
            } else {
                before + (chatId to existing.copy(references = existing.references - 1))
            }
            if (sessions.compareAndSet(before, after)) {
                if (existing.references <= 1) expected.shutdown()
                return
            }
        }
    }

    private fun nowMillis(): Long = currentEpochMillis()

    private fun requestTreeRefresh() {
        treeRefreshRequests.trySend(Unit)
    }

    private companion object {
        val TURN_LIFECYCLE_EVENTS = setOf("turn-started", "turn-finished")
    }

    private data class SessionEntry(
        val core: ChatSessionCore,
        val references: Int,
    )
}
