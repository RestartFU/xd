package com.restartfu.xd

import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.model.TreeSnapshot
import com.restartfu.xd.net.ConnectionActor
import com.restartfu.xd.net.Link
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.net.PlatformSocketFactory
import com.restartfu.xd.protocol.AgentAuthProviderReply
import com.restartfu.xd.protocol.AgentAuthReply
import com.restartfu.xd.protocol.AgentCatalogReply
import com.restartfu.xd.protocol.AgentCliProviderReply
import com.restartfu.xd.protocol.AgentCliUpdatesReply
import com.restartfu.xd.protocol.AgentSecretsReply
import com.restartfu.xd.protocol.AutoSettleSettingsReply
import com.restartfu.xd.protocol.BackendReply
import com.restartfu.xd.protocol.ChatTitleSettingsReply
import com.restartfu.xd.protocol.FolderContextReply
import com.restartfu.xd.protocol.ResumeInterruptedReply
import com.restartfu.xd.store.ProviderStates
import com.restartfu.xd.protocol.HostUpdateReply
import com.restartfu.xd.protocol.DirectoryListReply
import com.restartfu.xd.protocol.DoneReply
import com.restartfu.xd.protocol.FolderSettingsReply
import com.restartfu.xd.protocol.NewChatSourcesReply
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
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.flow.updateAndGet
import kotlinx.coroutines.flow.getAndUpdate
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.sync.Mutex
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.JsonPrimitive
import com.restartfu.xd.terminal.TerminalEvent
import com.restartfu.xd.terminal.TerminalWire

public class XdClient(
    socketFactory: PlatformSocketFactory,
    credentials: CredentialStore,
    private val scope: CoroutineScope,
) {
    private val actor = ConnectionActor(
        socketFactory,
        credentials,
        scope,
    )
    private val treeStore = TreeStore(actor)
    private val treeRefreshRequests = Channel<Unit>(Channel.CONFLATED)
    private val sessions = MutableStateFlow<Map<String, SessionEntry>>(emptyMap())

    public val link: StateFlow<Link> = actor.link
    public val hasCredentials: StateFlow<Boolean> = actor.hasCredentials
    public val credentialsReady: StateFlow<Boolean> = actor.credentialsReady
    public val tree: StateFlow<TreeSnapshot> = treeStore.state
    private val _sessionGeneration = MutableStateFlow(0L)
    /** Changes when forgetting a remote invalidates all retained chat handles. */
    public val sessionGeneration: StateFlow<Long> = _sessionGeneration.asStateFlow()

    private val pendingInteractions = MutableStateFlow<Map<String, Set<String>>>(emptyMap())
    private val _waitingChats = MutableStateFlow<Set<String>>(emptySet())
    private val unreadTurns = MutableStateFlow<Set<String>>(emptySet())
    private val unreadTerminalOwners = MutableStateFlow<Set<String>>(emptySet())
    private val terminalBusy = MutableStateFlow<Set<String>>(emptySet())
    private val visibleChat = MutableStateFlow<String?>(null)
    private val agentAuthStates = ProviderStates<AgentAuthProviderReply> { it.provider }
    private val agentCliStates = ProviderStates<AgentCliProviderReply> { it.provider }

    /** Each assistant's sign-in on the host, kept current by its broadcasts. */
    public val agentAuth: StateFlow<Map<String, AgentAuthProviderReply>?> = agentAuthStates.items

    /** Each assistant CLI's version and update state, kept current by its broadcasts. */
    public val agentClis: StateFlow<Map<String, AgentCliProviderReply>?> = agentCliStates.items

    private val _autoSettleSettings = MutableStateFlow<AutoSettleSettingsReply?>(null)
    /** Counts the host's auto-settle broadcasts, to tell whether one arrived during a request. */
    private val autoSettleBroadcasts = MutableStateFlow(0L)
    /** Something has asked for the rules on this host, so a reconnect should read them again. */
    private val autoSettleWanted = MutableStateFlow(false)
    private val _autoSettleStale = MutableStateFlow(false)
    /** Numbers every rules request, to order failures and successes. */
    private val autoSettleCalls = MutableStateFlow(0L)
    /** The newest request that failed, and the newest whose answer was shown. */
    private var autoSettleFailedCall = 0L
    private var autoSettleAppliedCall = 0L
    /** Guards the checks above together with the updates they allow. */
    private val autoSettleLock = Mutex()
    /**
     * A read shows the current rules but says nothing about whether a change
     * was made, so a failed change keeps its own error until a newer change
     * succeeds.
     */
    private var autoSettleWriteFailed = 0L
    private var autoSettleWriteSucceeded = 0L
    private val _autoSettleWriteError = MutableStateFlow<String?>(null)
    public val autoSettleWriteError: StateFlow<String?> = _autoSettleWriteError.asStateFlow()

    /** The person has seen the failed change; only their own Try again clears it without a newer change. */
    public suspend fun dismissAutoSettleWriteError() {
        autoSettleLock.withLock { _autoSettleWriteError.value = null }
    }
    private val _autoSettleError = MutableStateFlow<String?>(null)

    /**
     * Why the newest rules request failed, in the same request order as the
     * stale mark: an answer to a newer request clears it, and a failure
     * superseded by one never shows.
     */
    public val autoSettleError: StateFlow<String?> = _autoSettleError.asStateFlow()

    /**
     * The rules shown may be out of date: the host said they changed and the
     * read that followed failed. Cleared by the next read that lands.
     */
    public val autoSettleStale: StateFlow<Boolean> = _autoSettleStale.asStateFlow()

    /**
     * The host's auto-settle rules. Null until read, and again after
     * forgetting the host.
     *
     * A reply and a concurrently published broadcast can reach the socket in
     * either order (see the protocol's ordering caveat), so neither is trusted
     * over the other. A broadcast only says the rules changed: it starts a
     * fresh read, sent after it arrived and so reflecting at least that
     * change. A reply is kept only when no broadcast arrived while it was in
     * flight. Every change broadcasts, so the value always settles on the
     * host's.
     */
    public val autoSettleSettings: StateFlow<AutoSettleSettingsReply?> = _autoSettleSettings.asStateFlow()

    private val _autoSettleInFlight = MutableStateFlow(0)

    /**
     * Rules requests still waiting on the host, including the reads a
     * broadcast starts. While any is out the rules shown may be about to
     * change, so a client should not offer edits computed from them.
     */
    public val autoSettleInFlight: StateFlow<Int> = _autoSettleInFlight.asStateFlow()

    private fun rereadAutoSettle() {
        // Counted before launching, so there is no moment the read is due but not counted.
        _autoSettleInFlight.update { it + 1 }
        scope.launch { autoSettleCall(Ops.autoSettleSettings(), background = true, counted = true) }
    }

    /** Chats with an approval or question waiting on the user, across every chat. */
    public val waitingChats: StateFlow<Set<String>> = _waitingChats.asStateFlow()

    /** Chats whose turn finished while they were not open. */
    public val unreadChats: StateFlow<Set<String>> = combine(
        unreadTurns, unreadTerminalOwners, treeStore.state,
    ) { turns, owners, snapshot ->
        turns + owners.flatMap { treeStore.terminalChatIds(snapshot, it) }
    }.stateIn(scope, SharingStarted.Eagerly, emptySet())

    /** The displayed transcript; cached background tabs can still become unread. */
    public fun setVisibleChat(chatId: String?) {
        visibleChat.value = chatId
        if (chatId != null) markSeen(chatId)
    }

    public fun markSeen(chatId: String) {
        unreadTurns.update { it - chatId }
        unreadTerminalOwners.update { owners ->
            owners.filterNot { it == chatId || chatId in treeStore.terminalChatIds(it) }.toSet()
        }
    }

    private fun trackSignals(eventName: String?, chatId: String, event: kotlinx.serialization.json.JsonObject) {
        val requestId = (event["request_id"] as? JsonPrimitive)?.contentOrNull
        when (eventName) {
            "interaction" -> if (requestId != null) {
                pendingInteractions.update { it + (chatId to (it[chatId].orEmpty() + requestId)) }
            }
            "interaction-resolved" -> if (requestId != null) {
                pendingInteractions.update { it + (chatId to (it[chatId].orEmpty() - requestId)) }
            }
            "interactions-cleared" -> pendingInteractions.update { it - chatId }
            "turn-started" -> unreadTurns.update { it - chatId }
            "turn-finished" -> if (chatId != visibleChat.value) unreadTurns.update { it + chatId }
            // A terminal agent finishing is the same news as a chat turn finishing.
            "terminal-activity" -> {
                val working = (event["terminal_working"] as? JsonPrimitive)?.booleanOrNull ?: return
                val members = treeStore.terminalChatIds(chatId)
                val owner = treeStore.terminalOwner(chatId)
                if (working) {
                    terminalBusy.update { it + owner }
                    unreadTerminalOwners.update { owners ->
                        owners.filterNot { treeStore.terminalOwner(it) == owner }.toSet()
                    }
                } else {
                    val previouslyBusy = terminalBusy.getAndUpdate { it - owner - members }
                    val finished = owner in previouslyBusy || members.any { it in previouslyBusy }
                    if (finished && visibleChat.value !in members && visibleChat.value != chatId) {
                        unreadTerminalOwners.update { it + owner }
                    }
                }
            }
            else -> return
        }
        _waitingChats.value = pendingInteractions.value.filterValues { it.isNotEmpty() }.keys
    }

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
                    // Broadcasts sent while disconnected were missed.
                    if (autoSettleWanted.value) rereadAutoSettle()
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
                if (eventName == "agent-auth-changed") {
                    runCatching { event.value.decodeReply<AgentAuthProviderReply>() }
                        .onSuccess(agentAuthStates::event)
                }
                if (eventName == "agent-cli-update") {
                    runCatching { event.value.decodeReply<AgentCliProviderReply>() }
                        .onSuccess(agentCliStates::event)
                }
                if (eventName == "auto-settle-settings") {
                    autoSettleLock.withLock { autoSettleBroadcasts.update { it + 1 } }
                    rereadAutoSettle()
                }
                if (eventName == "tree") {
                    requestTreeRefresh()
                    sessions.value.values.forEach { entry ->
                        entry.core.requestReload()
                    }
                }
                val chatId = (event.value["chat"] as? JsonPrimitive)?.contentOrNull
                if (chatId != null) trackSignals(eventName, chatId, event.value)
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

    public suspend fun connect(connection: SshConnection): ConnectResult =
        actor.connect(connection)

    public suspend fun forget() {
        actor.forget()
        treeStore.clear()
        takeSessions().values.forEach { it.core.invalidate() }
        pendingInteractions.value = emptyMap()
        _waitingChats.value = emptySet()
        unreadTurns.value = emptySet()
        unreadTerminalOwners.value = emptySet()
        terminalBusy.value = emptySet()
        visibleChat.value = null
        _sessionGeneration.update { it + 1 }
        autoSettleLock.withLock {
            _autoSettleSettings.value = null
            autoSettleWanted.value = false
            _autoSettleStale.value = false
            autoSettleFailedCall = 0
            autoSettleAppliedCall = 0
            _autoSettleError.value = null
            autoSettleWriteFailed = 0
            autoSettleWriteSucceeded = 0
            _autoSettleWriteError.value = null
        }
        agentAuthStates.clear()
        agentCliStates.clear()
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
        if (
            !chatId.startsWith("global:") &&
            !chatId.startsWith("session:") &&
            chosen.state.value.title.isEmpty() &&
            actor.link.value is Link.Up
        ) {
            chosen.requestReload()
        }
        return ChatSession(chosen) { releaseChat(chatId, chosen) }
    }

    /**
     * Asks about, or performs, an update of the connected host
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

    /** Renames a session. Its chats keep their own titles. */
    public suspend fun renameSession(sessionId: String, title: String) {
        actor.call(Ops.renameSession(sessionId, title))
        requestTreeRefresh()
    }

    /** Deletes a session with every chat in it. There is no undo. */
    public suspend fun deleteSession(sessionId: String) {
        actor.call(Ops.deleteSession(sessionId))
        requestTreeRefresh()
    }

    public suspend fun settleSession(sessionId: String) {
        actor.call(Ops.settleSession(sessionId))
        requestTreeRefresh()
    }

    public suspend fun unsettleSession(sessionId: String) {
        actor.call(Ops.unsettleSession(sessionId))
        requestTreeRefresh()
    }

    public suspend fun setSessionAutoSettle(sessionId: String, enabled: Boolean) {
        actor.call(Ops.setSessionAutoSettle(sessionId, enabled))
        requestTreeRefresh()
    }

    /** Asks each assistant for its sign-in state, into [agentAuth]. */
    public suspend fun loadAgentAuth() {
        val generation = _sessionGeneration.value
        val mark = agentAuthStates.mark()
        val reply = actor.decodeReply(actor.call(Ops.agentAuth())) { it.decodeReply<AgentAuthReply>() }
        if (generation == _sessionGeneration.value) agentAuthStates.reply(reply.providers, mark)
    }

    /** Starts, feeds, or cancels a sign-in; progress arrives through [agentAuth]. */
    public suspend fun agentAuthAction(action: Ops.AuthAction, provider: String, input: String? = null) {
        actor.call(Ops.agentAuthAction(action, provider, input))
    }

    /** `status` or `check` the assistant CLIs, or `install` one [provider]'s update, into [agentClis]. */
    public suspend fun agentCliUpdates(action: String = "status", provider: String? = null) {
        val generation = _sessionGeneration.value
        val mark = agentCliStates.mark()
        val reply = actor.decodeReply(actor.call(Ops.agentCliUpdates(action, provider))) {
            it.decodeReply<AgentCliUpdatesReply>()
        }
        if (generation == _sessionGeneration.value) agentCliStates.reply(reply.providers, mark)
    }

    /** The host's assistants and their models. */
    public suspend fun agentCatalog(): List<BackendReply> =
        actor.decodeReply(actor.call(Ops.agentCatalog())) { it.decodeReply<AgentCatalogReply>().backends }

    public suspend fun chatTitleSettings(): ChatTitleSettingsReply =
        actor.decodeReply(actor.call(Ops.chatTitleSettings())) { it.decodeReply<ChatTitleSettingsReply>() }

    public suspend fun setChatTitlesEnabled(enabled: Boolean): ChatTitleSettingsReply =
        actor.decodeReply(actor.call(Ops.setChatTitlesEnabled(enabled))) { it.decodeReply<ChatTitleSettingsReply>() }

    public suspend fun setChatTitleModel(backend: String, model: String?): ChatTitleSettingsReply =
        actor.decodeReply(actor.call(Ops.setChatTitleModel(backend, model))) { it.decodeReply<ChatTitleSettingsReply>() }

    public suspend fun folderContext(folderId: String): String? =
        actor.decodeReply(actor.call(Ops.folderContext(folderId))) { it.decodeReply<FolderContextReply>() }.context

    public suspend fun setFolderContext(folderId: String, context: String?) {
        actor.call(Ops.setFolderContext(folderId, context))
        // A project's context reaches its chats' next turns; reload open ones.
        requestTreeRefresh()
    }

    public suspend fun agentSecrets(folderId: String? = null): List<String> =
        actor.decodeReply(actor.call(Ops.agentSecrets(folderId))) { it.decodeReply<AgentSecretsReply>() }.names

    public suspend fun setAgentSecrets(folderId: String?, entries: List<Ops.SecretEntry>) {
        actor.call(Ops.setAgentSecrets(folderId, entries))
    }

    /** Answers for chats a host restart cut off; see [Ops.resumeInterrupted]. */
    public suspend fun resumeInterrupted(resume: Boolean): List<String> =
        actor.decodeReply(actor.call(Ops.resumeInterrupted(resume))) { it.decodeReply<ResumeInterruptedReply>() }.resumed

    /** Reads the rules into [autoSettleSettings]. */
    public suspend fun loadAutoSettleSettings() {
        autoSettleCall(Ops.autoSettleSettings())
    }

    /** Changes only the rules given; [autoSettleSettings] then holds all of them. */
    public suspend fun setAutoSettleSettings(
        afterDays: Ops.AfterDays? = null,
        onMerge: Boolean? = null,
        keepPullRequests: Boolean? = null,
    ) {
        autoSettleCall(Ops.setAutoSettleSettings(afterDays, onMerge, keepPullRequests), write = true)
    }

    /**
     * A [background] read reports failure only by marking the rules stale,
     * and only when no newer request has been made since it started.
     */
    private suspend fun autoSettleCall(
        request: kotlinx.serialization.json.JsonObject,
        background: Boolean = false,
        counted: Boolean = false,
        write: Boolean = false,
    ) {
        if (!counted) _autoSettleInFlight.update { it + 1 }
        try {
            autoSettleWanted.value = true
            val generation = _sessionGeneration.value
            val broadcasts = autoSettleBroadcasts.value
            val call = autoSettleCalls.updateAndGet { it + 1 }
            val rules = try {
                val value = actor.call(request)
                actor.decodeReply(value) { it.decodeReply<AutoSettleSettingsReply>() }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                // Any failure, in the foreground or not, leaves the rules shown
                // possibly old until a newer request's answer is shown.
                // Only the newest failure counts: an older one finishing last
                // must not replace its error.
                autoSettleLock.withLock {
                    if (write && generation == _sessionGeneration.value && call > autoSettleWriteFailed) {
                        autoSettleWriteFailed = call
                        if (call > autoSettleWriteSucceeded) {
                            _autoSettleWriteError.value = error.message ?: "The change was not made"
                        }
                    }
                    if (generation == _sessionGeneration.value && call > autoSettleFailedCall) {
                        autoSettleFailedCall = call
                        if (call > autoSettleAppliedCall) {
                            _autoSettleStale.value = true
                            // A background read's failure is told by the stale mark alone.
                            if (!background) _autoSettleError.value = error.message ?: "The host did not answer"
                        }
                    }
                }
                if (!background) throw error
                return
            }
            // Otherwise the read that broadcast started decides the value.
            // Only an answer to a newer request than the one shown: an older
            // reply arriving last must not bring back older rules.
            autoSettleLock.withLock {
                if (write && generation == _sessionGeneration.value && call > autoSettleWriteSucceeded) {
                    autoSettleWriteSucceeded = call
                    if (call > autoSettleWriteFailed) _autoSettleWriteError.value = null
                }
                if (generation == _sessionGeneration.value && broadcasts == autoSettleBroadcasts.value &&
                    call > autoSettleAppliedCall
                ) {
                    _autoSettleSettings.value = rules
                    autoSettleAppliedCall = call
                    if (call > autoSettleFailedCall) {
                        _autoSettleStale.value = false
                        _autoSettleError.value = null
                    }
                }
            }
        } finally {
            _autoSettleInFlight.update { it - 1 }
        }
    }

    public suspend fun renameFolder(folderId: String, name: String) {
        actor.call(Ops.renameFolder(folderId, name))
        requestTreeRefresh()
    }

    /** Removes a project from xd; the host moves its files to the workspace trash. */
    public suspend fun trashFolder(folderId: String) {
        actor.call(Ops.trashFolder(folderId))
        requestTreeRefresh()
    }

    /** Branches and open pull requests a new session in [folderId] can start from. */
    public suspend fun newChatSources(folderId: String): NewChatSourcesReply {
        val value = actor.call(Ops.newChatSources(folderId))
        return actor.decodeReply(value) { it.decodeReply<NewChatSourcesReply>() }
    }

    /** A project's checkouts, and whether it can make another worktree. */
    public suspend fun folderSettings(folderId: String): FolderSettingsReply {
        val value = actor.call(Ops.folderSettings(folderId))
        return actor.decodeReply(value) { it.decodeReply<FolderSettingsReply>() }
    }

    public suspend fun createChat(
        folderId: String,
        title: String?,
        backend: String? = null,
        sessionId: String? = null,
        worktree: Ops.NewSessionWorktree? = null,
    ): String {
        val value = actor.call(Ops.newChat(folderId, title, backend, sessionId, worktree))
        val id = actor.decodeReply(value) {
            val reply = it.decodeReply<DoneReply>()
            reply.id?.takeIf(String::isNotBlank)
                ?: throw RemoteProtocolException("New chat reply has no id")
        }
        requestTreeRefresh()
        return id
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
