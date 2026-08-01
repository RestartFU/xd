package com.restartfu.xd

import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.model.TreeSnapshot
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.net.Link
import com.restartfu.xd.net.PairResult
import com.restartfu.xd.net.PlatformSocketFactory
import com.restartfu.xd.protocol.DoneReply
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.RemoteProtocolException
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
import kotlinx.serialization.json.JsonPrimitive
import com.restartfu.xd.terminal.TerminalEvent
import com.restartfu.xd.terminal.TerminalWire

public class XdClient(
    socketFactory: PlatformSocketFactory,
    credentials: CredentialStore,
    private val scope: CoroutineScope,
) {
    private val actor = ConnectionActor(socketFactory, credentials, scope)
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
                }
                val chatId = (event.value["chat"] as? JsonPrimitive)?.contentOrNull
                if (chatId != null && eventName in TURN_LIFECYCLE_EVENTS) {
                    treeStore.setChatWorking(
                        chatId = chatId,
                        working = eventName == "turn-started",
                        sequence = event.sequence,
                    )
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
        deviceName: String,
    ): PairResult = actor.pair(host, port, code, deviceName)

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

    public suspend fun createChat(
        folderId: String,
        title: String?,
    ): String {
        val value = actor.call(Ops.newChat(folderId, title))
        return actor.decodeReply(value) {
            val reply = it.decodeReply<DoneReply>()
            reply.id?.takeIf(String::isNotBlank)
                ?: throw RemoteProtocolException("New chat reply has no id")
        }
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
