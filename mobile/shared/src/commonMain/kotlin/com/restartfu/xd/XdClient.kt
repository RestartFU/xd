package com.restartfu.xd

import com.restartfu.xd.credentials.CredentialStore
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.model.ChatQuotaSettings
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
import com.restartfu.xd.protocol.PauseAllReply
import com.restartfu.xd.protocol.ResumeAllReply
import com.restartfu.xd.protocol.ResumeInterruptedReply
import com.restartfu.xd.protocol.UsageAccountReply
import com.restartfu.xd.protocol.UsagePriceReply
import com.restartfu.xd.protocol.UsagePricesReply
import com.restartfu.xd.protocol.UsageSummaryReply
import com.restartfu.xd.protocol.WireJson
import com.restartfu.xd.store.ProviderStates
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.longOrNull
import kotlin.io.encoding.Base64
import kotlin.io.encoding.ExperimentalEncodingApi
import com.restartfu.xd.protocol.HostUpdateReply
import com.restartfu.xd.protocol.DirectoryListReply
import com.restartfu.xd.protocol.DoneReply
import com.restartfu.xd.protocol.FolderSettingsReply
import com.restartfu.xd.protocol.LinkPullRequestsReply
import com.restartfu.xd.protocol.UnlinkPullRequestReply
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
import kotlinx.serialization.json.intOrNull
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

    private val _ownerIcons = MutableStateFlow<Map<String, ByteArray?>>(emptyMap())

    /**
     * Avatars of the GitHub accounts folders name as their `owner`, as PNG or
     * JPEG bytes. An account the host answered for without an avatar maps to
     * null; one not answered for yet is absent.
     */
    public val ownerIcons: StateFlow<Map<String, ByteArray?>> = _ownerIcons.asStateFlow()

    /** Owners asked about and not answered for yet. */
    private val ownerIconsRequested = MutableStateFlow<Set<String>>(emptySet())

    private val pendingInteractions = MutableStateFlow<Map<String, Set<String>>>(emptyMap())
    private val _waitingChats = MutableStateFlow<Set<String>>(emptySet())
    private val unreadTurns = MutableStateFlow<Set<String>>(emptySet())
    private val unreadTerminalOwners = MutableStateFlow<Set<String>>(emptySet())
    private val terminalBusy = MutableStateFlow<Set<String>>(emptySet())
    private val visibleChat = MutableStateFlow<String?>(null)
    private val agentAuthStates = ProviderStates<AgentAuthProviderReply> { it.provider }
    private val agentCliStates = ProviderStates<AgentCliProviderReply> { it.provider }
    /** Asked for on this host, so a reconnect reads them again even if no answer landed. */
    private val agentAuthWanted = MutableStateFlow(false)
    private val agentClisWanted = MutableStateFlow(false)
    private val agentAuthFreshness = Freshness()
    private val agentClisFreshness = Freshness()

    /** Sign-in state may be out of date: the newest read of it failed. */
    public val agentAuthStale: StateFlow<Boolean> = agentAuthFreshness.stale

    /** CLI state may be out of date: the newest read of it failed. */
    public val agentClisStale: StateFlow<Boolean> = agentClisFreshness.stale

    /**
     * Whether state shown may be out of date, with failures and answers
     * ordered by request: an older answer cannot clear a newer failure, and
     * an older failure cannot mark a newer answer stale.
     */
    private class Freshness {
        private val lock = Mutex()
        private var calls = 0L
        private var failed = 0L
        private var applied = 0L
        private val _stale = MutableStateFlow(false)
        val stale: StateFlow<Boolean> = _stale.asStateFlow()

        suspend fun start(): Long = lock.withLock { ++calls }

        suspend fun failed(call: Long) = lock.withLock {
            failed = maxOf(failed, call)
            if (call > applied) _stale.value = true
        }

        suspend fun applied(call: Long) = lock.withLock {
            applied = maxOf(applied, call)
            if (call > failed) _stale.value = false
        }

        suspend fun reset() = lock.withLock {
            failed = 0
            applied = 0
            _stale.value = false
        }
    }

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

    private val _chatQuotaSettings = MutableStateFlow<ChatQuotaSettings?>(null)

    /**
     * The host's default chat quota. Null until read, and again after
     * forgetting the host.
     *
     * Each change is broadcast with the new value, and broadcasts arrive in
     * the order the host made the changes, so a broadcast is applied as it
     * comes. A reply can reach the socket before or after a broadcast sent
     * while it was out (see the protocol's ordering caveat), so it is kept
     * only when no broadcast arrived in the meantime.
     */
    public val chatQuotaSettings: StateFlow<ChatQuotaSettings?> = _chatQuotaSettings.asStateFlow()
    private val chatQuotaLock = Mutex()
    private var chatQuotaBroadcasts = 0L
    private var chatQuotaCalls = 0L
    private var chatQuotaApplied = 0L
    /** Something has asked for the default on this host, so a reconnect reads it again. */
    private var chatQuotaWanted = false

    /** Reads the host's default chat quota into [chatQuotaSettings]. */
    public suspend fun loadChatQuotaSettings(): ChatQuotaSettings =
        chatQuotaCall(Ops.chatQuotaSettings(), requested = ChatQuotaSettings(null))

    /** Sets the host's default chat quota; null limits no chat. */
    public suspend fun setChatQuotaSettings(percent: Int?): ChatQuotaSettings =
        chatQuotaCall(Ops.setChatQuotaSettings(percent), requested = ChatQuotaSettings(percent))

    /**
     * [requested] stands in for a reply that does not name the default: the
     * value a change asked for, or no limit for a read.
     */
    private suspend fun chatQuotaCall(
        request: kotlinx.serialization.json.JsonObject,
        requested: ChatQuotaSettings,
    ): ChatQuotaSettings {
        val generation = _sessionGeneration.value
        val (call, broadcasts) = chatQuotaLock.withLock {
            chatQuotaWanted = true
            ++chatQuotaCalls to chatQuotaBroadcasts
        }
        val settings = ChatQuotaSettings.parse(actor.call(request)) ?: requested
        chatQuotaLock.withLock {
            if (generation == _sessionGeneration.value && broadcasts == chatQuotaBroadcasts && call > chatQuotaApplied) {
                _chatQuotaSettings.value = settings
                chatQuotaApplied = call
            }
        }
        return settings
    }

    private suspend fun chatQuotaBroadcast(event: kotlinx.serialization.json.JsonObject) {
        val settings = ChatQuotaSettings.parse(event)
        chatQuotaLock.withLock {
            chatQuotaBroadcasts++
            if (settings != null) _chatQuotaSettings.value = settings
        }
        // One that does not say what changed is read again.
        if (settings == null) rereadChatQuota()
    }

    private fun rereadChatQuota() {
        scope.launch { runCatching { loadChatQuotaSettings() } }
    }

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
            treeStore.state.collect { requestOwnerIcons(it) }
        }
        scope.launch {
            actor.link.collect { value ->
                if (value is Link.Up) {
                    // A pending avatar's event may have been lost with the
                    // connection, so anything unanswered is asked for again.
                    ownerIconsRequested.value = emptySet()
                    requestOwnerIcons(treeStore.state.value)
                    // Broadcasts sent while disconnected were missed.
                    if (autoSettleWanted.value) rereadAutoSettle()
                    if (chatQuotaLock.withLock { chatQuotaWanted }) rereadChatQuota()
                    // A failure is not dropped: what is shown may be out of
                    // date, and the next read that lands clears the mark.
                    if (agentAuthWanted.value) {
                        scope.launch { runCatching { loadAgentAuth() } }
                    }
                    if (agentClisWanted.value) {
                        scope.launch { runCatching { agentCliUpdates("status") } }
                    }
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
                        .onSuccess { agentAuthStates.event(it) }
                }
                if (eventName == "agent-cli-update") {
                    runCatching { event.value.decodeReply<AgentCliProviderReply>() }
                        .onSuccess { agentCliStates.event(it) }
                }
                if (eventName == "auto-settle-settings") {
                    autoSettleLock.withLock { autoSettleBroadcasts.update { it + 1 } }
                    rereadAutoSettle()
                }
                if (eventName == "chat-quota-settings") chatQuotaBroadcast(event.value)
                if (eventName == "owner-icon") {
                    (event.value["owner"] as? JsonPrimitive)?.contentOrNull?.let { owner ->
                        recordOwnerIcon(owner, event.value)
                    }
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
                if (chatId != null && eventName == "background-tasks") {
                    val count = (event.value["background_tasks"] as? JsonPrimitive)?.intOrNull
                    if (count != null) {
                        treeStore.setChatBackgroundTasks(chatId, count, event.sequence)
                    }
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
        _ownerIcons.value = emptyMap()
        ownerIconsRequested.value = emptySet()
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
        chatQuotaLock.withLock {
            _chatQuotaSettings.value = null
            chatQuotaWanted = false
            chatQuotaApplied = chatQuotaCalls
        }
        agentAuthStates.clear()
        agentCliStates.clear()
        agentAuthWanted.value = false
        agentClisWanted.value = false
        agentAuthFreshness.reset()
        agentClisFreshness.reset()
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

    /**
     * Links pull requests to [chatId]; see [Ops.linkPullRequests]. The list
     * itself comes back through [tree], which the host broadcasts.
     */
    public suspend fun linkPullRequests(chatId: String, urls: List<String>): LinkPullRequestsReply {
        val value = actor.call(Ops.linkPullRequests(chatId, urls))
        val reply = actor.decodeReply(value) { it.decodeReply<LinkPullRequestsReply>() }
        if (reply.linked.isNotEmpty()) requestTreeRefresh()
        return reply
    }

    /** Removes a linked pull request from [chatId]; true when it was linked. */
    public suspend fun unlinkPullRequest(chatId: String, url: String): Boolean {
        val value = actor.call(Ops.unlinkPullRequest(chatId, url))
        val changed = actor.decodeReply(value) { it.decodeReply<UnlinkPullRequestReply>() }.changed
        if (changed) requestTreeRefresh()
        return changed
    }

    /** Asks each assistant for its sign-in state, into [agentAuth]. */
    public suspend fun loadAgentAuth() {
        agentAuthWanted.value = true
        val generation = _sessionGeneration.value
        val call = agentAuthFreshness.start()
        val mark = agentAuthStates.mark()
        val reply = try {
            actor.decodeReply(actor.call(Ops.agentAuth())) { it.decodeReply<AgentAuthReply>() }
        } catch (error: CancellationException) {
            throw error
        } catch (error: Throwable) {
            if (generation == _sessionGeneration.value) agentAuthFreshness.failed(call)
            throw error
        }
        if (generation == _sessionGeneration.value) {
            agentAuthStates.reply(reply.providers, mark)
            agentAuthFreshness.applied(call)
        }
    }

    /**
     * Starts, feeds, or cancels a sign-in, adds an account, or signs one out;
     * progress arrives through [agentAuth].
     */
    public suspend fun agentAuthAction(
        action: Ops.AuthAction,
        provider: String,
        input: String? = null,
        account: String? = null,
    ) {
        actor.call(Ops.agentAuthAction(action, provider, input, account))
    }

    /** Sets the order turns use [provider]'s accounts in; the change arrives through [agentAuth]. */
    public suspend fun agentAccountsOrder(provider: String, accounts: List<String>) {
        actor.call(Ops.agentAccountsOrder(provider, accounts))
    }

    /** Turns automatic account switching on or off; the change arrives through [agentAuth]. */
    public suspend fun agentAccountsAutoSwitch(provider: String, enabled: Boolean) {
        actor.call(Ops.agentAccountsAutoSwitch(provider, enabled))
    }

    /** `status` or `check` the assistant CLIs, or `install` one [provider]'s update, into [agentClis]. */
    public suspend fun agentCliUpdates(action: String = "status", provider: String? = null) {
        agentClisWanted.value = true
        val generation = _sessionGeneration.value
        val call = agentClisFreshness.start()
        val mark = agentCliStates.mark()
        val reply = try {
            actor.decodeReply(actor.call(Ops.agentCliUpdates(action, provider))) {
                it.decodeReply<AgentCliUpdatesReply>()
            }
        } catch (error: CancellationException) {
            throw error
        } catch (error: Throwable) {
            if (generation == _sessionGeneration.value) agentClisFreshness.failed(call)
            throw error
        }
        if (generation == _sessionGeneration.value) {
            agentCliStates.reply(reply.providers, mark)
            agentClisFreshness.applied(call)
        }
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

    /** Stops every working chat in the folder and marks it paused; answers the chats it stopped. */
    public suspend fun pauseAll(folderId: String): List<String> {
        val paused = actor.decodeReply(actor.call(Ops.pauseAll(folderId))) { it.decodeReply<PauseAllReply>() }.paused
        // The host broadcasts `tree` too; this brings the paused marks in either way.
        requestTreeRefresh()
        return paused
    }

    /** Continues every paused chat in the folder; answers the chats it continued. */
    public suspend fun resumeAll(folderId: String): List<String> {
        val resumed = actor.decodeReply(actor.call(Ops.resumeAll(folderId))) { it.decodeReply<ResumeAllReply>() }.resumed
        requestTreeRefresh()
        return resumed
    }

    /** Answers for chats a host restart cut off; see [Ops.resumeInterrupted]. */
    public suspend fun resumeInterrupted(resume: Boolean): List<String> =
        actor.decodeReply(actor.call(Ops.resumeInterrupted(resume))) { it.decodeReply<ResumeInterruptedReply>() }.resumed

    private val usageGenerations = MutableStateFlow(0L)

    /**
     * Sends a request the host answers with an event to this connection only,
     * matched by [match], and waits for that event. Subscribed before sending,
     * so the answer cannot slip past; a dropped connection loses it, which
     * the timeout reports.
     */
    private suspend fun awaitAnswer(
        request: kotlinx.serialization.json.JsonObject,
        match: (kotlinx.serialization.json.JsonObject) -> Boolean,
    ): kotlinx.serialization.json.JsonObject = coroutineScope {
        val answer = async(start = CoroutineStart.UNDISPATCHED) {
            actor.events.first { match(it.value) }.value
        }
        try {
            actor.call(request)
            withTimeout(USAGE_TIMEOUT_MILLIS) { answer.await() }
        } catch (error: kotlinx.coroutines.TimeoutCancellationException) {
            answer.cancel()
            throw RemoteProtocolException("The host did not answer in time. Try again.")
        } catch (error: Throwable) {
            answer.cancel()
            throw error
        }
    }

    private fun failure(value: kotlinx.serialization.json.JsonObject): String? =
        (value["error"] as? JsonPrimitive)?.contentOrNull

    /** Cost and tokens over [days], read on the host. [refresh] fetches prices again. */
    public suspend fun usageSummary(days: Int, timeZone: String, refresh: Boolean = false): UsageSummaryReply {
        val generation = usageGenerations.updateAndGet { it + 1 }
        val event = awaitAnswer(Ops.usageSummary(days, timeZone, generation, refresh)) {
            (it["event"] as? JsonPrimitive)?.contentOrNull == "usage-summary" &&
                (it["generation"] as? JsonPrimitive)?.longOrNull == generation
        }
        failure(event)?.let { throw RemoteProtocolException(it) }
        val summary = event["summary"] as? kotlinx.serialization.json.JsonObject
            ?: throw RemoteProtocolException("The usage answer had no summary")
        return WireJson.decodeFromJsonElement(UsageSummaryReply.serializer(), summary)
    }

    /** Each signed-in assistant account's subscription limits. */
    public suspend fun usageLimits(timeZone: String): List<UsageAccountReply> {
        val generation = usageGenerations.updateAndGet { it + 1 }
        val event = awaitAnswer(Ops.usageLimits(timeZone, generation)) {
            (it["event"] as? JsonPrimitive)?.contentOrNull == "usage-limits" &&
                (it["generation"] as? JsonPrimitive)?.longOrNull == generation
        }
        failure(event)?.let { throw RemoteProtocolException(it) }
        val accounts = event["accounts"] ?: return emptyList()
        return WireJson.decodeFromJsonElement(ListSerializer(UsageAccountReply.serializer()), accounts)
    }

    /**
     * Spends one Codex reset credit and answers the host's outcome: `reset`,
     * `nothingToReset`, `noCredit`, or `alreadyRedeemed`.
     */
    public suspend fun consumeResetCredit(accountKey: String, idempotencyKey: String): String {
        val event = awaitAnswer(Ops.usageConsumeResetCredit(accountKey, idempotencyKey)) {
            // Only this attempt's answer: an older attempt's late error must not
            // end this wait. Hosts that sent errors without the key time out
            // instead, and the attempt stays to be checked again.
            (it["event"] as? JsonPrimitive)?.contentOrNull == "usage-reset-credit" &&
                (it["idempotency_key"] as? JsonPrimitive)?.contentOrNull == idempotencyKey
        }
        failure(event)?.let { throw RemoteProtocolException(it) }
        return (event["outcome"] as? JsonPrimitive)?.contentOrNull
            ?: throw RemoteProtocolException("The reset answer had no outcome")
    }

    public suspend fun usagePrices(): UsagePricesReply =
        actor.decodeReply(actor.call(Ops.usagePrices())) { it.decodeReply<UsagePricesReply>() }

    /** Without [aliases] the host keeps its saved mappings, so another client's are not reverted. */
    public suspend fun setUsagePrices(prices: Map<String, UsagePriceReply>, aliases: Map<String, String>? = null): UsagePricesReply =
        actor.decodeReply(actor.call(Ops.setUsagePrices(prices, aliases))) { it.decodeReply<UsagePricesReply>() }

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
        startFromRemote: Boolean = true,
        effort: String? = null,
        access: String? = null,
        plan: Boolean? = null,
    ): String {
        val value = actor.call(Ops.newChat(folderId, title, backend, sessionId, worktree, startFromRemote, effort, access, plan))
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
        group: Boolean = false,
        owner: String? = null,
    ): String {
        val value = actor.call(
            Ops.newFolder(name, parentId, repository, repositoryUrl, group, owner),
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

    /** Asks the host for the avatar of each owner in [snapshot] not asked about yet. */
    private fun requestOwnerIcons(snapshot: TreeSnapshot) {
        val generation = _sessionGeneration.value
        val owners = snapshot.folders.mapNotNullTo(mutableSetOf()) { it.owner }
        for (owner in owners) {
            var claimed = false
            ownerIconsRequested.update { asked ->
                claimed = owner !in asked && owner !in _ownerIcons.value
                if (claimed) asked + owner else asked
            }
            if (!claimed) continue
            scope.launch {
                val reply = try {
                    actor.call(Ops.ownerIcon(owner))
                } catch (error: CancellationException) {
                    throw error
                } catch (_: Throwable) {
                    // The folder keeps its icon; the next tree asks again.
                    ownerIconsRequested.update { it - owner }
                    return@launch
                }
                // An answer from a host this client has since forgotten is dropped.
                if (generation == _sessionGeneration.value) recordOwnerIcon(owner, reply)
            }
        }
    }

    /**
     * Records the host's answer about [owner]'s avatar, from a reply or an
     * `owner-icon` event. A pending answer waits for the event; a failed fetch
     * is asked about again with the next tree, and the host's own cooldown
     * keeps that from reaching GitHub every time.
     */
    @OptIn(ExperimentalEncodingApi::class)
    private fun recordOwnerIcon(owner: String, body: kotlinx.serialization.json.JsonObject) {
        if ((body["pending"] as? JsonPrimitive)?.booleanOrNull == true) return
        if ((body["retry"] as? JsonPrimitive)?.booleanOrNull == true) {
            ownerIconsRequested.update { it - owner }
            return
        }
        val mime = (body["mime"] as? JsonPrimitive)?.contentOrNull
        val data = (body["data"] as? JsonPrimitive)?.contentOrNull
        val icon = data?.takeIf { mime == "image/png" || mime == "image/jpeg" }
            ?.let { runCatching { Base64.decode(it) }.getOrNull() }
            ?.takeIf(ByteArray::isNotEmpty)
        _ownerIcons.update { it + (owner to icon) }
        ownerIconsRequested.update { it - owner }
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
        /** Reading 90 days of transcripts can take a while on a busy host. */
        const val USAGE_TIMEOUT_MILLIS = 120_000L
        val TURN_LIFECYCLE_EVENTS = setOf("turn-started", "turn-finished")
    }

    private data class SessionEntry(
        val core: ChatSessionCore,
        val references: Int,
    )
}
