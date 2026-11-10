package com.restartfu.xd.mobile

import android.app.Application
import android.net.Uri
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import com.restartfu.xd.credentials.parsePairingInvitation
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.model.DirectAgent
import com.restartfu.xd.protocol.AgentAccountReply
import com.restartfu.xd.protocol.AgentAuthProviderReply
import com.restartfu.xd.protocol.AutoSettleSettingsReply
import com.restartfu.xd.protocol.BackendReply
import com.restartfu.xd.net.Link
import com.restartfu.xd.protocol.ChatTitleSettingsReply
import com.restartfu.xd.model.ChatQuotaSettings
import com.restartfu.xd.protocol.ChatOption
import com.restartfu.xd.protocol.HostUpdateReply
import com.restartfu.xd.protocol.Limits
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.mobile.ui.NOT_A_PULL_REQUEST_LINK
import com.restartfu.xd.model.pastedPullRequestLinks
import com.restartfu.xd.store.ChatSession
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.withContext

data class ShortcutEditorState(
    val folderId: String?,
    val title: String,
    val prompts: List<String> = emptyList(),
    val loading: Boolean = true,
    val saving: Boolean = false,
    val error: String? = null,
    /**
     * Saving replaces the whole list, so it waits until the current list has
     * arrived: otherwise a failed load would let Save erase shortcuts nobody saw.
     */
    val loaded: Boolean = false,
    /** Identifies one opening, so a late reply never revives a closed or reopened editor. */
    val opening: Long = 0,
    /**
     * The list being edited. Kept here rather than in the dialog, so it
     * outlives recreating the activity and belongs to this opening alone.
     */
    val draft: List<DraftPrompt> = emptyList(),
    /** The next row's id; only ever grows, so a removed row's id is never reused. */
    val nextId: Long = 0,
)

/**
 * One prompt in the shortcut editor. The [id] keeps a row's identity while
 * others are added or removed, so a focused field stays on its own prompt.
 */
data class DraftPrompt(val id: Long, val text: String)

/** The most prompt buttons one editor offers. */
const val MAX_SHORTCUTS = 24

/** A session the user just settled, which the app offers to bring back. */
data class SettledNotice(
    val sessionId: String,
    val title: String,
    val serial: Long,
    /** The paired host it was settled on; Undo goes nowhere else. */
    val generation: Long = 0,
)

/** The queue as a queued message was opened on, and the host's revision of it. */
data class OpenedQueue(val messages: List<String>, val revision: Long?)

/** A queued message open for editing: its place, its text, and the queue around it. */
data class OpenedMessage(
    val index: Int,
    val text: String,
    val queue: OpenedQueue,
    /** The edit in progress, kept here so no message text goes into saved state. */
    val draft: String = text,
)

data class CreatedDirectSession(
    val projectId: String,
    val chatId: String,
    val agent: DirectAgent,
    val title: String,
    val sessionId: String? = null,
)

data class PendingHostKeyConfirmation(
    val hostKey: SshHostKey,
    val host: String,
    val port: Int,
    val username: String,
)

class MainViewModel @JvmOverloads constructor(
    application: Application,
    /** The app's host connection; screen tests pass one answering from a script. */
    val client: XdClient = (application as XdApplication).client,
) : AndroidViewModel(application) {
    private val _connecting = MutableStateFlow(false)
    private val _pendingHostKey = MutableStateFlow<PendingHostKeyConfirmation?>(null)
    private val _privateKeyName = MutableStateFlow<String?>(null)
    private var pendingConnection: SshConnection? = null
    private var shortcutOpenings = 0L
    private var privateKeyBytes: ByteArray? = null
    private val _forgetting = MutableStateFlow(false)
    private val _creatingChat = MutableStateFlow(false)
    private val _creatingWorkspace = MutableStateFlow(false)
    private val _createdDirectSession = MutableStateFlow<CreatedDirectSession?>(null)
    private val _deletingChat = MutableStateFlow(false)
    private val _renamingChat = MutableStateFlow(false)
    private val _pausingAll = MutableStateFlow(false)
    private val _error = MutableStateFlow<String?>(null)
    private val _shortcutEditor = MutableStateFlow<ShortcutEditorState?>(null)

    val connecting: StateFlow<Boolean> = _connecting.asStateFlow()
    val pendingHostKey: StateFlow<PendingHostKeyConfirmation?> = _pendingHostKey.asStateFlow()
    val privateKeyName: StateFlow<String?> = _privateKeyName.asStateFlow()
    val createdDirectSession: StateFlow<CreatedDirectSession?> =
        _createdDirectSession.asStateFlow()
    val error: StateFlow<String?> = _error.asStateFlow()
    val creatingWorkspace: StateFlow<Boolean> = _creatingWorkspace.asStateFlow()
    val deletingChat: StateFlow<Boolean> = _deletingChat.asStateFlow()
    val renamingChat: StateFlow<Boolean> = _renamingChat.asStateFlow()
    /** A pause-all or resume-all request is on its way, so the button waits. */
    val pausingAll: StateFlow<Boolean> = _pausingAll.asStateFlow()
    private val _settledNotice = MutableStateFlow<SettledNotice?>(null)
    private var settledSerial = 0L
    val settledNotice: StateFlow<SettledNotice?> = _settledNotice.asStateFlow()
    /** Auto-settle requests still waiting on the host, including reads a broadcast started. */
    val autoSettleBusy: StateFlow<Int> = client.autoSettleInFlight

    /** The connected host's rules; the client keeps them current and clears them on forgetting. */
    val autoSettle: StateFlow<AutoSettleSettingsReply?> = client.autoSettleSettings
    /** The client orders failures and answers by request, so a superseded failure never shows. */
    val autoSettleError: StateFlow<String?> = client.autoSettleError
    /** A change that failed, kept until a newer change succeeds; a read does not clear it. */
    val autoSettleWriteError: StateFlow<String?> = client.autoSettleWriteError
    val autoSettleStale: StateFlow<Boolean> = client.autoSettleStale

    /** The host's default chat quota; the client keeps it current and clears it on forgetting. */
    val chatQuota: StateFlow<ChatQuotaSettings?> = client.chatQuotaSettings
    private val _chatQuotaError = MutableStateFlow<String?>(null)
    /** Why the newest chat quota request failed; cleared by the next one that lands. */
    val chatQuotaError: StateFlow<String?> = _chatQuotaError.asStateFlow()
    private var chatQuotaRequests = 0L

    private val _accountsError = MutableStateFlow<String?>(null)
    val accountsError: StateFlow<String?> = _accountsError.asStateFlow()
    val agentAuth = client.agentAuth
    val agentClis = client.agentClis
    val agentAuthStale = client.agentAuthStale
    val agentClisStale = client.agentClisStale
    private val _chatTitles = MutableStateFlow<ChatTitleSettingsReply?>(null)
    private val _chatTitlesError = MutableStateFlow<String?>(null)
    private val _catalog = MutableStateFlow<List<BackendReply>>(emptyList())
    private val _catalogError = MutableStateFlow<String?>(null)
    private var catalogReads = 0L
    /** Why the models could not be listed; the last list stays meanwhile. */
    val hostCatalogError: StateFlow<String?> = _catalogError.asStateFlow()
    val chatTitles: StateFlow<ChatTitleSettingsReply?> = _chatTitles.asStateFlow()
    val chatTitlesError: StateFlow<String?> = _chatTitlesError.asStateFlow()
    private val _chatTitlesWriteError = MutableStateFlow<String?>(null)
    /** A change that failed, kept until a newer change succeeds; a read does not clear it. */
    val chatTitlesWriteError: StateFlow<String?> = _chatTitlesWriteError.asStateFlow()
    private var chatTitleWrites = 0L
    private var chatTitleWriteSucceeded = 0L
    private var chatTitleWriteFailed = 0L
    private var accountActions = 0L
    private val _signInDrafts = MutableStateFlow<Map<String, SignInDraft>>(emptyMap())
    /**
     * Each assistant's sign-in code being typed and whether it is out, kept
     * here so recreating the activity neither loses a code nor sends it twice.
     */
    val signInDrafts: StateFlow<Map<String, SignInDraft>> = _signInDrafts.asStateFlow()
    private var accountActionSucceeded = 0L
    private var accountActionFailed = 0L
    /** Each assistant's models, for choosing its naming model. */
    val hostCatalog: StateFlow<List<BackendReply>> = _catalog.asStateFlow()
    private var chatTitleRequests = 0L
    /** Settings has asked for the title settings, so a reconnect reads them again. */
    private var chatTitlesWanted = false

    init {
        // Answer for chats a host restart cut off, when this phone is set to
        // continue them. With the setting off it does not answer at all, so
        // another device set to resume them still can.
        viewModelScope.launch {
            var wasUp = false
            client.link.collect { link ->
                val up = link is Link.Up
                // A change made elsewhere while this phone was away was not broadcast to it.
                if (up && !wasUp && chatTitlesWanted) loadChatTitles()
                if (up && !wasUp && (application as XdApplication).settings.resumeChats.value) {
                    // Off the watcher, so a reconnect during the request is still seen.
                    launch {
                        try {
                            client.resumeInterrupted(resume = true)
                        } catch (error: CancellationException) {
                            throw error
                        } catch (error: Throwable) {
                            // The play button on each chat still continues it.
                        }
                    }
                }
                wasUp = up
            }
        }
        // A code belongs to one sign-in: once that ends, whether or not its
        // dialog is open, the code goes, and the next attempt starts with none.
        viewModelScope.launch {
            client.agentAuth.collect {
                _signInDrafts.update { drafts -> drafts.filter { (name, draft) -> draft.sending || signInLive(name, draft) } }
            }
        }
        // Per-host state belongs to the host it came from.
        viewModelScope.launch {
            client.sessionGeneration.collect {
                _accountsError.value = null
                _signInDrafts.value = emptyMap()
                _chatTitles.value = null
                _chatTitlesError.value = null
                _chatTitlesWriteError.value = null
                _catalog.value = emptyList()
                _catalogError.value = null
                catalogReads++
                chatTitleRequests++
                chatTitlesWanted = false
                _chatQuotaError.value = null
                chatQuotaRequests++
            }
        }
    }

    /**
     * Runs one assistant-account action, reporting a failure in the accounts
     * dialog until a newer action succeeds. An older failure arriving late, or
     * one from a host since forgotten, is set aside.
     */
    private fun accountsAction(failure: String, done: (Boolean) -> Unit = {}, action: suspend () -> Unit) {
        val serial = ++accountActions
        val generation = client.sessionGeneration.value
        viewModelScope.launch {
            try {
                action()
                done(true)
                if (generation != client.sessionGeneration.value) return@launch
                accountActionSucceeded = maxOf(accountActionSucceeded, serial)
                if (serial > accountActionFailed) _accountsError.value = null
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                done(false)
                // Nor does it replace a newer action's failure.
                if (generation != client.sessionGeneration.value || serial < accountActionSucceeded || serial < accountActionFailed) return@launch
                accountActionFailed = serial
                _accountsError.value = error.message ?: failure
            }
        }
    }

    /**
     * Reads both lists again. A failed read shows as the client's stale flags,
     * which order failures and answers by request, so a late failure never
     * hides a newer answer.
     */
    fun loadAccounts() {
        viewModelScope.launch { quietly { client.loadAgentAuth() } }
        viewModelScope.launch { quietly { client.agentCliUpdates("status") } }
    }

    /** Pressed by the person: reads again, and acknowledges a failed action. */
    fun retryAccounts() {
        _accountsError.value = null
        loadAccounts()
    }

    private suspend fun quietly(read: suspend () -> Unit) {
        try {
            read()
        } catch (error: CancellationException) {
            throw error
        } catch (error: Throwable) {
            // Reported through the stale flags.
        }
    }

    fun checkCliUpdates() = accountsAction("Could not check for updates") { client.agentCliUpdates("check") }

    fun installCliUpdate(provider: String) =
        accountsAction("Could not start the update") { client.agentCliUpdates("install", provider) }

    /** Signs [account] in again in its own home; the user's own sign-in when left out. */
    fun signIn(provider: String, account: String? = null) =
        accountsAction("Could not start signing in") {
            client.agentAuthAction(Ops.AuthAction.START, provider, account = account?.takeIf { it != AgentAccountReply.DEFAULT })
        }

    /** Signs a new account in, in a new home; it joins the list once that succeeds. */
    fun addAccount(provider: String) =
        accountsAction("Could not start adding an account") { client.agentAuthAction(Ops.AuthAction.ADD, provider) }

    /** Signs the user's own CLI sign-in out. */
    fun signOut(provider: String) =
        accountsAction("Could not sign out") { client.agentAuthAction(Ops.AuthAction.LOGOUT, provider) }

    /** Signs an added account out and deletes its saved sign-in from the host. */
    fun removeAccount(provider: String, account: String) {
        if (account == AgentAccountReply.DEFAULT) return
        accountsAction("Could not remove the account") {
            client.agentAuthAction(Ops.AuthAction.LOGOUT, provider, account = account)
        }
    }

    /** Moves the ranked account at [index] one place up or down in the order turns use. */
    fun moveAccount(provider: AgentAuthProviderReply, index: Int, up: Boolean) {
        val order = provider.orderMoving(index, up) ?: return
        accountsAction("Could not change the order") { client.agentAccountsOrder(provider.provider, order) }
    }

    fun setAccountAutoSwitch(provider: String, enabled: Boolean) =
        accountsAction("Could not change account switching") { client.agentAccountsAutoSwitch(provider, enabled) }

    fun editSignInInput(provider: AgentAuthProviderReply, text: String) {
        val attempt = provider.signInAttempt()
        _signInDrafts.update { drafts ->
            val draft = drafts[provider.provider]?.takeIf { it.attempt == attempt } ?: SignInDraft(attempt = attempt)
            if (draft.sending) drafts else drafts + (provider.provider to draft.copy(text = text))
        }
    }

    /** Whether [draft] still belongs to the sign-in the host is running for [provider]. */
    private fun signInLive(provider: String, draft: SignInDraft): Boolean =
        client.agentAuth.value?.get(provider)
            ?.let { it.signingIn() != null && it.signInAttempt() == draft.attempt } == true

    /** Sends the typed code, keeping it until the host takes it, so a failure can be sent again. */
    fun sendSignInInput(provider: AgentAuthProviderReply) {
        val attempt = provider.signInAttempt()
        val draft = _signInDrafts.value[provider.provider]?.takeIf { it.attempt == attempt } ?: return
        val sent = draft.text.trim()
        if (draft.sending || sent.isEmpty()) return
        val name = provider.provider
        _signInDrafts.update { it + (name to draft.copy(sending = true)) }
        val generation = client.sessionGeneration.value
        accountsAction(
            "Could not send that",
            done = { taken ->
                // Forgotten with its host meanwhile: the drafts now are another host's.
                if (generation == client.sessionGeneration.value) _signInDrafts.update { drafts ->
                    val current = drafts[name]?.takeIf { it.attempt == attempt } ?: return@update drafts
                    // A failed code is kept to send again, while its sign-in lasts.
                    if (taken || !signInLive(name, current)) drafts - name
                    else drafts + (name to current.copy(sending = false))
                }
            },
        ) { client.agentAuthAction(Ops.AuthAction.INPUT, name, sent) }
    }

    fun cancelSignIn(provider: String) =
        accountsAction("Could not cancel signing in") { client.agentAuthAction(Ops.AuthAction.CANCEL, provider) }

    /**
     * Only the newest request's answer, or a read's failure, is shown. A
     * change's failure is kept apart, until a newer change succeeds, since a
     * read answering afterwards says nothing of whether the change was made.
     */
    private fun chatTitleRequest(failure: String, write: Boolean = false, request: suspend () -> ChatTitleSettingsReply) {
        val serial = ++chatTitleRequests
        val writeSerial = if (write) ++chatTitleWrites else 0L
        val generation = client.sessionGeneration.value
        _chatTitlesError.value = null
        viewModelScope.launch {
            try {
                val reply = request()
                if (generation != client.sessionGeneration.value) return@launch
                if (write) {
                    chatTitleWriteSucceeded = maxOf(chatTitleWriteSucceeded, writeSerial)
                    if (writeSerial > chatTitleWriteFailed) _chatTitlesWriteError.value = null
                }
                if (serial == chatTitleRequests) _chatTitles.value = reply
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (generation != client.sessionGeneration.value) return@launch
                if (write && writeSerial > chatTitleWriteSucceeded && writeSerial > chatTitleWriteFailed) {
                    chatTitleWriteFailed = writeSerial
                    _chatTitlesWriteError.value = error.message ?: failure
                }
                if (serial != chatTitleRequests) return@launch
                if (!write) {
                    _chatTitlesError.value = error.message ?: failure
                    return@launch
                }
                // An earlier change may have landed even though this one did
                // not, and its answer was set aside for this one; ask again.
                try {
                    val fresh = client.chatTitleSettings()
                    if (serial == chatTitleRequests) _chatTitles.value = fresh
                } catch (error: CancellationException) {
                    throw error
                } catch (error: Throwable) {
                    // Keep the error shown; the next request reads again.
                }
            }
        }
    }

    fun loadChatTitles() {
        chatTitlesWanted = true
        chatTitleRequest("Could not read the chat title settings") { client.chatTitleSettings() }
        loadCreationCatalog()
    }

    fun loadCreationCatalog() {
        val read = ++catalogReads
        _catalogError.value = null
        viewModelScope.launch {
            try {
                val catalog = client.agentCatalog()
                if (read == catalogReads) _catalog.value = catalog
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (read == catalogReads) _catalogError.value = error.message ?: "Could not list the models"
            }
        }
    }

    /** Pressed by the person, it also acknowledges a failed change. */
    fun retryChatTitles() {
        _chatTitlesWriteError.value = null
        loadChatTitles()
    }

    fun setChatTitlesEnabled(enabled: Boolean) =
        chatTitleRequest("Could not change chat titles", write = true) { client.setChatTitlesEnabled(enabled) }

    /** Null goes back to the host's built-in choice, which follows its default as that changes. */
    fun setChatTitleModel(backend: String, model: String?) =
        chatTitleRequest("Could not change the naming model", write = true) { client.setChatTitleModel(backend, model) }

    fun loadChatQuota() = chatQuotaRequest("Could not read the chat quota") { client.loadChatQuotaSettings() }

    /** Sets the host's default chat quota; null limits no chat. */
    fun setChatQuota(percent: Int?) =
        chatQuotaRequest("Could not change the chat quota") { client.setChatQuotaSettings(percent) }

    /** The value arrives through [chatQuota]; only the newest request's failure is shown. */
    private fun chatQuotaRequest(failure: String, request: suspend () -> Unit) {
        val serial = ++chatQuotaRequests
        val generation = client.sessionGeneration.value
        viewModelScope.launch {
            try {
                request()
                if (serial == chatQuotaRequests && generation == client.sessionGeneration.value) _chatQuotaError.value = null
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (serial == chatQuotaRequests && generation == client.sessionGeneration.value) {
                    _chatQuotaError.value = error.message ?: failure
                }
            }
        }
    }

    init {
        viewModelScope.launch {
            client.sessionGeneration.collect {
                // An Undo offered for another host must not reach this one.
                _settledNotice.value = null
            }
        }
    }
    val shortcutEditor: StateFlow<ShortcutEditorState?> = _shortcutEditor.asStateFlow()
    private val _host = MutableStateFlow<HostUpdateReply?>(null)
    private val _hostError = MutableStateFlow<String?>(null)
    private val _updating = MutableStateFlow(false)
    val host: StateFlow<HostUpdateReply?> = _host.asStateFlow()
    val hostError: StateFlow<String?> = _hostError.asStateFlow()
    val hostBusy: StateFlow<Boolean> = _updating.asStateFlow()

    fun connectPairing(address: String, pairingCode: String) {
        try {
            val invitation = parsePairingInvitation(address, pairingCode)
            val fingerprint = invitation.fingerprint
            val pin = fingerprint.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
            startConnection(SshConnection(
                host = invitation.host, port = invitation.port, username = android.os.Build.MODEL.take(100),
                authentication = SshAuthentication.Paired(fingerprint = fingerprint, code = invitation.code),
                hostKey = SshHostKey("TLS-SHA256", pin, fingerprint),
            ))
        } catch (error: Throwable) { _error.value = error.message ?: "Invalid pairing code" }
    }

    fun connect(
        host: String,
        port: Int,
        username: String,
        password: String,
        usePrivateKey: Boolean,
        passphrase: String,
    ) {
        val authentication = if (usePrivateKey) {
            val key = privateKeyBytes
            if (key == null) {
                _error.value = "Choose an SSH private key"
                return
            }
            SshAuthentication.PrivateKey(key.copyOf(), passphrase.ifEmpty { null })
        } else {
            SshAuthentication.Password(password)
        }
        startConnection(SshConnection(host, port, username, authentication))
    }

    fun importPrivateKey(uri: Uri, displayName: String?) {
        viewModelScope.launch {
            try {
                val bytes = withContext(Dispatchers.IO) {
                    getApplication<Application>().contentResolver.openInputStream(uri)?.use {
                        it.readBytes()
                    } ?: error("Could not open the selected private key")
                }
                require(bytes.isNotEmpty()) { "The selected private key is empty" }
                clearPrivateKey()
                privateKeyBytes = bytes
                _privateKeyName.value = displayName ?: "Imported private key"
                _error.value = null
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not import the private key"
            }
        }
    }

    fun clearPrivateKey() {
        privateKeyBytes?.fill(0)
        privateKeyBytes = null
        _privateKeyName.value = null
    }

    fun confirmHostKey() {
        val connection = pendingConnection ?: return
        _pendingHostKey.value = null
        pendingConnection = null
        startConnection(connection)
    }

    fun cancelHostKeyConfirmation() {
        clearPendingConnection()
    }

    private fun startConnection(connection: SshConnection) {
        if (_connecting.value) return
        clearPendingConnection()
        _connecting.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                when (val result = client.connect(connection)) {
                    is ConnectResult.Success -> clearPrivateKey()
                    is ConnectResult.HostKeyVerificationRequired -> {
                        pendingConnection = connection.copy(hostKey = result.hostKey)
                        _pendingHostKey.value = PendingHostKeyConfirmation(
                            hostKey = result.hostKey,
                            host = connection.host,
                            port = connection.port,
                            username = connection.username,
                        )
                    }
                    is ConnectResult.Failure -> _error.value = result.message
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not connect over SSH"
            } finally {
                _connecting.value = false
            }
        }
    }

    private fun clearPendingConnection() {
        val connection = pendingConnection
        pendingConnection = null
        val authentication = connection?.authentication
        if (authentication is SshAuthentication.PrivateKey) {
            authentication.bytes.fill(0)
        }
        _pendingHostKey.value = null
    }

    fun forget() {
        if (_forgetting.value) return
        _forgetting.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                client.forget()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not forget the remote"
            } finally {
                _forgetting.value = false
            }
        }
    }

    fun createDirectSession(
        folderId: String,
        title: String,
        agent: DirectAgent,
        sessionId: String? = null,
        worktree: Ops.NewSessionWorktree? = null,
        startFromRemote: Boolean = true,
        effort: String? = null,
        access: String? = null,
        plan: Boolean? = null,
    ) {
        if (_creatingChat.value) return
        _creatingChat.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                val chatId = client.createChat(
                    folderId = folderId,
                    title = title.takeIf(String::isNotBlank),
                    backend = agent.wire,
                    sessionId = sessionId,
                    worktree = worktree,
                    effort = effort, access = access, plan = plan,
                    startFromRemote = startFromRemote,
                )
                getApplication<Application>().getSharedPreferences("new-chat-drafts", android.content.Context.MODE_PRIVATE)
                    .edit().remove("$folderId/${sessionId ?: "new"}").apply()
                _createdDirectSession.value = CreatedDirectSession(
                    folderId,
                    chatId,
                    agent,
                    title.takeIf(String::isNotBlank) ?: if (sessionId == null) "New Session" else "New chat",
                    sessionId,
                )
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not create a session"
            } finally {
                _creatingChat.value = false
            }
        }
    }

    fun consumeCreatedDirectSession(session: CreatedDirectSession) {
        if (_createdDirectSession.value == session) _createdDirectSession.value = null
    }

    fun createWorkspace(name: String, repository: String? = null, parentId: String? = null) =
        createInTree("Could not create the workspace") {
            client.createFolder(name = name, parentId = parentId, repository = repository)
        }

    /** Makes a folder of projects, showing [owner]'s avatar when one is named. */
    fun createProjectFolder(name: String, parentId: String? = null, owner: String? = null) =
        createInTree("Could not create the folder") {
            client.createFolder(name = name, parentId = parentId, group = true, owner = owner)
        }

    private fun createInTree(failure: String, create: suspend () -> Unit) {
        if (_creatingWorkspace.value) return
        _creatingWorkspace.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                create()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: failure
            } finally {
                _creatingWorkspace.value = false
            }
        }
    }

    fun deleteChat(chatId: String) {
        if (_deletingChat.value) return
        _deletingChat.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                client.deleteChat(chatId)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not delete the chat"
            } finally {
                _deletingChat.value = false
            }
        }
    }

    fun renameChat(chatId: String, title: String) {
        if (_renamingChat.value) return
        _renamingChat.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                client.renameChat(chatId, title)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not rename the chat"
            } finally {
                _renamingChat.value = false
            }
        }
    }

    /** Runs one tree change on the host, reporting a refusal where the app shows errors. */
    private fun treeAction(failure: String, action: suspend () -> Unit) {
        _error.value = null
        viewModelScope.launch {
            try {
                action()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: failure
            }
        }
    }

    /**
     * Moves a session to the Settled shelf. The host refuses while one of its
     * chats is running, has a queued message, or waits on an approval, and
     * says why.
     */
    fun settleSession(sessionId: String, title: String) = treeAction("Could not settle the session") {
        val generation = client.sessionGeneration.value
        client.settleSession(sessionId)
        if (generation == client.sessionGeneration.value) {
            _settledNotice.value = SettledNotice(sessionId, title, ++settledSerial, generation)
        }
    }

    /** Brings back what [notice] settled, only on the host it was settled on. */
    fun undoSettle(notice: SettledNotice) {
        if (notice.generation == client.sessionGeneration.value) unsettleSession(notice.sessionId)
    }

    fun consumeSettledNotice(notice: SettledNotice) {
        _settledNotice.compareAndSet(notice, null)
    }

    fun unsettleSession(sessionId: String) = treeAction("Could not bring the session back") {
        client.unsettleSession(sessionId)
    }

    fun setSessionAutoSettle(sessionId: String, enabled: Boolean) =
        treeAction("Could not change auto-settle for the session") {
            client.setSessionAutoSettle(sessionId, enabled)
        }

    fun renameSession(sessionId: String, title: String) = treeAction("Could not rename the session") {
        client.renameSession(sessionId, title)
    }

    fun deleteSession(sessionId: String) = treeAction("Could not delete the session") {
        client.deleteSession(sessionId)
    }

    /** Stops every working chat in project [folderId], for stepping away from work. */
    fun pauseProject(folderId: String) =
        pauseOrResumeAll("Could not pause the chats") { client.pauseAll(folderId) }

    /** Continues every chat in project [folderId] that [pauseProject] stopped. */
    fun resumeProject(folderId: String) =
        pauseOrResumeAll("Could not resume the chats") { client.resumeAll(folderId) }

    private fun pauseOrResumeAll(failure: String, action: suspend () -> Unit) {
        if (_pausingAll.value) return
        _pausingAll.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                action()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: failure
            } finally {
                _pausingAll.value = false
            }
        }
    }

    fun renameProject(folderId: String, name: String) = treeAction("Could not rename the project") {
        client.renameFolder(folderId, name)
    }

    fun deleteProject(folderId: String) = treeAction("Could not delete the project") {
        client.trashFolder(folderId)
    }

    fun renameProjectFolder(folderId: String, name: String) = treeAction("Could not rename the folder") {
        client.renameFolder(folderId, name)
    }

    /** Removes a folder and everything in it; the host moves the files to the workspace trash. */
    fun deleteProjectFolder(folderId: String) = treeAction("Could not delete the folder") {
        client.trashFolder(folderId)
    }

    /** Moves a project or folder into [parentId], or to the top level when it is null. */
    fun moveIntoFolder(folderId: String, parentId: String?) = treeAction("Could not move it") {
        client.moveFolder(folderId, parentId)
    }

    /** Reads the rules on opening Settings, leaving any failed change shown. */
    fun refreshAutoSettle() = autoSettleRequest("Could not read the auto-settle rules") {
        client.loadAutoSettleSettings()
    }

    /** Reads the rules again; pressed by the person, it also acknowledges a failed change. */
    fun loadAutoSettle() = autoSettleRequest("Could not read the auto-settle rules") {
        client.dismissAutoSettleWriteError()
        client.loadAutoSettleSettings()
    }

    /** Changes one auto-settle rule; the host answers with all of them. */
    fun setAutoSettle(
        afterDays: Ops.AfterDays? = null,
        onMerge: Boolean? = null,
        keepPullRequests: Boolean? = null,
    ) = autoSettleRequest("Could not change the auto-settle rules") {
        client.setAutoSettleSettings(afterDays = afterDays, onMerge = onMerge, keepPullRequests = keepPullRequests)
    }

    /**
     * The rules themselves arrive through [autoSettle]; this only reports
     * whether the newest request failed, so an older failure cannot cover a
     * later success.
     */
    /** The client records the outcome, in request order, through [autoSettleError]. */
    private fun autoSettleRequest(failure: String, request: suspend () -> Unit) {
        viewModelScope.launch {
            try {
                request()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                // Already reported by the client as $failure, unless superseded.
            }
        }
    }

    fun openShortcutEditor(folderId: String?, title: String) {
        val opened = ShortcutEditorState(folderId = folderId, title = title, opening = ++shortcutOpenings)
        _shortcutEditor.value = opened
        viewModelScope.launch {
            val update: ShortcutEditorState = try {
                val reply = client.shortcuts(folderId)
                val prompts = if (folderId == null) reply.global else reply.workspace
                opened.copy(
                    prompts = prompts,
                    draft = prompts.mapIndexed { index, text -> DraftPrompt(index.toLong(), text) },
                    nextId = prompts.size.toLong(),
                    loading = false,
                    loaded = true,
                )
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                opened.copy(loading = false, error = error.message ?: "Could not load shortcuts")
            }
            _shortcutEditor.update { current -> if (current?.opening == opened.opening) update else current }
        }
    }

    /**
     * One change to the open editor's draft, applied to the draft as it is
     * now rather than to the copy a callback last saw, so quick edits never
     * undo each other. Ignored while saving, which sends the draft as it was.
     */
    private fun changeShortcutDraft(opening: Long, change: (ShortcutEditorState) -> ShortcutEditorState) {
        _shortcutEditor.update { current ->
            if (current?.opening == opening && !current.saving) change(current) else current
        }
    }

    fun setShortcutText(opening: Long, id: Long, text: String) = changeShortcutDraft(opening) { editor ->
        editor.copy(draft = editor.draft.map { if (it.id == id) it.copy(text = text) else it })
    }

    fun addShortcut(opening: Long) = changeShortcutDraft(opening) { editor ->
        if (editor.draft.size >= MAX_SHORTCUTS) editor
        else editor.copy(draft = editor.draft + DraftPrompt(editor.nextId, ""), nextId = editor.nextId + 1)
    }

    fun removeShortcut(opening: Long, id: Long) = changeShortcutDraft(opening) { editor ->
        editor.copy(draft = editor.draft.filterNot { it.id == id })
    }

    fun closeShortcutEditor() {
        _shortcutEditor.value = null
    }

    /** Saves the open editor's current draft. */
    fun saveShortcuts() {
        val editor = _shortcutEditor.value ?: return
        if (editor.saving || !editor.loaded) return
        val prompts = editor.draft.map { it.text.trim() }.filter { it.isNotEmpty() }
        val saving = editor.copy(saving = true, error = null)
        _shortcutEditor.value = saving
        viewModelScope.launch {
            try {
                client.setShortcuts(editor.folderId, prompts)
                _shortcutEditor.update { current -> if (current?.opening == editor.opening) null else current }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _shortcutEditor.update { current ->
                    if (current?.opening == editor.opening) {
                        saving.copy(saving = false, error = error.message ?: "Could not save shortcuts")
                    } else {
                        current
                    }
                }
            }
        }
    }

    /**
     * Drives the host update from Settings.
     *
     * Install and restart are separate calls because they differ in cost:
     * replacing files is safe while turns run, restarting drops every attached
     * device and loses the turn.
     */
    fun hostUpdate(action: String = "status") {
        if (action == "check") _hostCheckPending.value = true
        // One request at a time; one asked for meanwhile runs next rather
        // than being dropped, so a reopened dialog's check is not lost.
        if (_updating.value) {
            // The same check already out answers for both.
            if (!(action == "check" && runningHostAction == "check")) queuedHostAction = action
            return
        }
        _updating.value = true
        runningHostAction = action
        // A new attempt replaces the last failure; the dialog shows an error in
        // place of any status, so a stale one would hide what happened next.
        _hostError.value = null
        viewModelScope.launch {
            try {
                _host.value = client.hostUpdate(action)
                if (action == "check") _hostCheckPending.value = false
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _hostError.value = error.message ?: "Could not reach the host"
            } finally {
                _updating.value = false
                queuedHostAction?.let { next ->
                    queuedHostAction = null
                    hostUpdate(next)
                }
            }
        }
    }

    private var queuedHostAction: String? = null
    private var runningHostAction: String? = null

    /**
     * A check was asked for and no answer has confirmed the host took it.
     * Until one does, following up asks for the check again, since a status
     * read alone would report the old result as up to date.
     */
    private val _hostCheckPending = MutableStateFlow(false)
    val hostCheckPending: StateFlow<Boolean> = _hostCheckPending.asStateFlow()

    /** Asks again about the update in progress, or the check that may not have happened. */
    fun followHostUpdate() {
        // A follow-up never queues behind a request already out: that request's
        // answer is the follow-up, and queuing would keep checks going forever.
        if (_updating.value) return
        hostUpdate(if (_hostCheckPending.value) "check" else "status")
    }

    /** Closing the dialog keeps an unconfirmed check, so reopening still follows it up. */
    fun clearHostUpdate() {
        _host.value = null
        _hostError.value = null
    }
}

class ChatViewModel(
    val client: XdClient,
    private val chatId: String,
) : ViewModel() {
    val session: ChatSession = client.openChat(chatId)
    val state = session.state
    private val _sending = MutableStateFlow(false)
    private val _cancelling = MutableStateFlow(false)
    private val _droppingQueued = MutableStateFlow(false)
    private val _steering = MutableStateFlow(false)
    private val _draft = MutableStateFlow("")
    private val _attachments = MutableStateFlow(emptyList<Attachment>())
    private val _attachmentError = MutableStateFlow<String?>(null)
    private val _queueNotice = MutableStateFlow<String?>(null)
    private val _openQueued = MutableStateFlow<OpenedMessage?>(null)

    /**
     * Whether the chat on screen has landed on its newest message. Cleared
     * when the reader leaves the chat, so going back into it lands again, and
     * kept through a rotation, which restores where they had scrolled to.
     */
    var landed = false

    /**
     * The queued message being edited, with the queue it was opened on. Kept
     * here, not in saved state: it survives rotation, and a long queue never
     * has to fit in the activity's saved state.
     */
    val openQueued: StateFlow<OpenedMessage?> = _openQueued.asStateFlow()

    fun openQueuedMessage(index: Int) {
        val state = state.value
        val text = state.queue.getOrNull(index) ?: return
        _openQueued.value = OpenedMessage(index, text, OpenedQueue(state.queue, state.queueRevision))
    }

    fun closeQueuedMessage() {
        _openQueued.value = null
    }

    fun editQueuedDraft(draft: String) {
        _openQueued.update { it?.copy(draft = draft) }
    }

    /** Why a queued-message action was not done, shown beside the queue. */
    val queueNotice: StateFlow<String?> = _queueNotice.asStateFlow()
    val attachments: StateFlow<List<Attachment>> = _attachments.asStateFlow()
    val attachmentError: StateFlow<String?> = _attachmentError.asStateFlow()
    val sending: StateFlow<Boolean> = _sending.asStateFlow()
    val cancelling: StateFlow<Boolean> = _cancelling.asStateFlow()
    val steering: StateFlow<Boolean> = _steering.asStateFlow()
    val queueBusy: StateFlow<Boolean> = _droppingQueued.asStateFlow()
    private val _catalog = MutableStateFlow(emptyList<BackendReply>())
    private val _catalogLoading = MutableStateFlow(false)
    private val _catalogError = MutableStateFlow<String?>(null)
    private val _selectingModel = MutableStateFlow(false)
    private val _modelSelectionError = MutableStateFlow<String?>(null)
    val catalog: StateFlow<List<BackendReply>> = _catalog.asStateFlow()
    val catalogLoading: StateFlow<Boolean> = _catalogLoading.asStateFlow()
    val catalogError: StateFlow<String?> = _catalogError.asStateFlow()
    val selectingModel: StateFlow<Boolean> = _selectingModel.asStateFlow()
    val modelSelectionError: StateFlow<String?> = _modelSelectionError.asStateFlow()
    val draft: StateFlow<String> = _draft.asStateFlow()
    private var draftSyncJob: Job? = null
    private var draftTextDirty = false
    private var draftAttachmentsDirty = false
    private var draftRevision = -1L

    init {
        loadCatalog()
        viewModelScope.launch {
            state.collect { synced ->
                if (synced.draftRevision <= draftRevision) return@collect
                draftRevision = synced.draftRevision
                if (!draftTextDirty) _draft.value = synced.draft
                if (
                    !draftAttachmentsDirty &&
                    !sameImages(_attachments.value, synced.draftAttachments)
                ) {
                    val revision = synced.draftRevision
                    val prepared = buildList {
                        for (png in synced.draftAttachments) {
                            add(ImageAttachments.fromPng(png))
                        }
                    }
                    if (state.value.draftRevision == revision && !draftAttachmentsDirty) {
                        _attachments.value = prepared
                    }
                }
            }
        }
    }

    fun approve(requestId: String, decision: String) {
        viewModelScope.launch { runCatching { session.approve(requestId, decision) } }
    }

    /** Agents asked to stop or move to the background, until their record changes. */
    private val _subagentActions = MutableStateFlow(emptyMap<String, String>())
    val subagentActions: StateFlow<Map<String, String>> = _subagentActions

    fun controlSubagent(key: String, action: String) {
        val label = if (action == "stop") "Stopping" else "Moving to background"
        _subagentActions.value = _subagentActions.value + (key to label)
        viewModelScope.launch {
            runCatching { session.controlSubagent(key, action) }
                .onFailure { _subagentActions.value = _subagentActions.value - key }
        }
    }

    /** Forgets a pending action once the agent's record shows it took effect. */
    fun settleSubagentAction(key: String) {
        if (key in _subagentActions.value) _subagentActions.value = _subagentActions.value - key
    }

    /**
     * Agents opened as their own tabs inside this chat, by record key. Only
     * kept while the chat is: an agent tab is a view, not a chat of its own.
     */
    private val _agentTabs = MutableStateFlow(emptyList<String>())
    val agentTabs: StateFlow<List<String>> = _agentTabs.asStateFlow()

    /** The agent tab shown in place of the conversation, if one is. */
    private val _viewingAgent = MutableStateFlow<String?>(null)
    val viewingAgent: StateFlow<String?> = _viewingAgent.asStateFlow()

    /** What the reader is writing to each agent, by record key. */
    private val _agentDrafts = MutableStateFlow(emptyMap<String, String>())
    val agentDrafts: StateFlow<Map<String, String>> = _agentDrafts.asStateFlow()
    private val _messagingAgent = MutableStateFlow(false)
    val messagingAgent: StateFlow<Boolean> = _messagingAgent.asStateFlow()

    fun openAgent(key: String) {
        if (key !in _agentTabs.value) _agentTabs.value = _agentTabs.value + key
        _viewingAgent.value = key
    }

    fun showChat() {
        _viewingAgent.value = null
    }

    fun closeAgent(key: String) {
        _agentTabs.value = _agentTabs.value - key
        _agentDrafts.update { it - key }
        if (_viewingAgent.value == key) _viewingAgent.value = null
    }

    fun updateAgentDraft(key: String, text: String) {
        _agentDrafts.update { it + (key to text) }
    }

    /**
     * Sends the reader's message to an agent. The host cannot write to an agent
     * itself, so it goes to this chat for the lead to pass on, steered into the
     * running turn when there is one rather than waiting behind it.
     */
    fun messageAgent(run: com.restartfu.xd.model.SubagentRun) {
        val key = run.key ?: return
        val address = run.messageAddress?.takeIf { run.canTakeMessages } ?: return
        val text = _agentDrafts.value[key].orEmpty().trim()
        if (text.isEmpty()) return
        val description = run.detail.ifBlank { run.identity }
        launchGuarded(_messagingAgent) {
            session.send(
                com.restartfu.xd.model.AgentRelay.format(description, address, text),
                steer = state.value.working,
            )
            _agentDrafts.update { drafts -> if (drafts[key]?.trim() == text) drafts - key else drafts }
        }
    }
    fun answer(requestId: String, answers: Map<String, String>) {
        viewModelScope.launch { runCatching { session.answer(requestId, answers) } }
    }

    fun updateDraft(value: String) {
        _draft.value = value
        scheduleDraftSync()
    }

    fun attach(context: android.content.Context, uris: List<android.net.Uri>) {
        if (uris.isEmpty()) return
        viewModelScope.launch {
            val room = Limits.MAX_IMAGES - _attachments.value.size
            if (room <= 0) {
                _attachmentError.value = "A message can carry at most 4 images"
                return@launch
            }
            val loaded = mutableListOf<Attachment>()
            for (uri in uris.take(room)) {
                try {
                    loaded += ImageAttachments.load(context, uri)
                } catch (error: CancellationException) {
                    throw error
                } catch (error: Throwable) {
                    _attachmentError.value = error.message ?: "That image could not be attached"
                }
            }
            if (loaded.isNotEmpty()) {
                _attachments.value = _attachments.value + loaded
                scheduleDraftSync(attachments = true)
            }
        }
    }

    fun removeAttachment(index: Int) {
        _attachments.value = _attachments.value.filterIndexed { at, _ -> at != index }
        scheduleDraftSync(attachments = true)
    }

    fun clearAttachmentError() {
        _attachmentError.value = null
    }

    fun send() {
        val text = _draft.value
        val images = _attachments.value
        launchGuarded(_sending) {
            session.send(text, images.map(Attachment::png))
            // Only clear what was actually sent: the composer may have moved
            // on while the request was in flight. Images go by content, one
            // per image sent: one swapped in meanwhile was never sent, and a
            // draft sync rebuilds the ones that were as new objects.
            val left = _attachments.value.toMutableList()
            for (sent in images) {
                val at = left.indexOfFirst { it.png.bytes.contentEquals(sent.png.bytes) }
                if (at >= 0) left.removeAt(at)
            }
            val textSent = _draft.value == text
            if (textSent) _draft.value = ""
            if (left.size != _attachments.value.size) _attachments.value = left
            if (textSent || images.isNotEmpty()) scheduleDraftSync(attachments = images.isNotEmpty())
        }
    }

    /**
     * Answers a tagged question with one of its options.
     *
     * Sent as its own message rather than through the draft: the reader may
     * have typed something they still mean to say, and an answer should not
     * carry it along or throw it away.
     */
    fun answer(option: String) {
        launchGuarded(_sending) { session.send(option) }
    }

    /** Sends a configured prompt without replacing or clearing the draft. */
    fun shortcut(prompt: String) {
        launchGuarded(_sending) { session.send(prompt) }
    }

    fun cancel() {
        val before = state.value
        launchGuarded(_cancelling) {
            session.cancel()
            withTimeoutOrNull(CANCEL_EVENT_TIMEOUT_MILLIS) {
                state.first {
                    !it.working || it.startedAtMillis != before.startedAtMillis
                }
            }
        }
    }

    /**
     * Queues the draft behind the running turn.
     *
     * The queue op carries text alone, so a draft with images goes as a send
     * instead: the host queues a message that arrives mid-turn with its images
     * saved and referenced, which is how the desktop queues one too.
     */
    fun enqueue() {
        if (_attachments.value.isNotEmpty()) return send()
        val text = _draft.value
        launchGuarded(_sending) {
            session.enqueue(text)
            if (_draft.value == text) updateDraft("")
        }
    }

    /**
     * Loads the catalog once per chat screen.
     *
     * It is host state rather than app state: hard-coding the models would
     * drift the moment one is added or retired, and set-option validates the
     * id, so a stale client would simply be refused.
     */
    fun loadCatalog() {
        if (_catalog.value.isNotEmpty() || _catalogLoading.value) return
        _catalogLoading.value = true
        _catalogError.value = null
        viewModelScope.launch {
            try {
                _catalog.value = session.catalog()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _catalogError.value = error.message ?: "Could not read the model list"
            } finally {
                _catalogLoading.value = false
            }
        }
    }

    fun setEffort(effort: String) {
        launchGuarded(_selectingModel) {
            session.setOption(ChatOption.EFFORT, effort)
        }
    }

    fun setAccess(access: String) {
        launchGuarded(_selectingModel) {
            session.setOption(ChatOption.ACCESS, access)
        }
    }

    /**
     * Plan and Build are one setting: Build is simply Plan off.
     *
     * The host overrides access while planning, so the access choice is not
     * meaningful until this is off again.
     */
    fun setPlan(planning: Boolean) {
        launchGuarded(_selectingModel) {
            session.setBoolOption(ChatOption.PLAN, planning)
        }
    }

    fun setFast(enabled: Boolean) {
        launchGuarded(_selectingModel) {
            session.setBoolOption(ChatOption.FAST, enabled)
        }
    }

    fun setNewWorktree(enabled: Boolean) {
        launchGuarded(_selectingModel) {
            session.setBoolOption(ChatOption.NEW_WORKTREE, enabled)
        }
    }

    fun selectModel(model: String) {
        if (_selectingModel.value) return
        _selectingModel.value = true
        _modelSelectionError.value = null
        viewModelScope.launch {
            try {
                session.selectModel(model)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _modelSelectionError.value = error.message ?: "Could not change the model. Try again."
            } finally {
                _selectingModel.value = false
            }
        }
    }

    fun clearModelSelectionError() {
        _modelSelectionError.value = null
    }

    fun clearCatalogError() {
        _catalogError.value = null
    }

    fun editQueued(index: Int, oldText: String, text: String, opened: OpenedQueue) {
        if (!sameQueue(state.value.queue, opened) || opened.revision == null) return
        launchGuarded(_droppingQueued) {
            session.editQueued(index, oldText, text, opened.revision)
        }
    }

    /**
     * Redirects the running turn to a queued message, editing it first when
     * the text changed.
     *
     * Both happen in one coroutine because the host matches the steer
     * against what is actually queued: sending them concurrently would race,
     * and the steer would be refused for not matching an edit that had not
     * landed yet.
     *
     * The host may promote the message and cancel the turn, so this waits for
     * the turn to change or the queue to shrink rather than reporting success
     * while the old one is still winding down.
     */
    fun steerQueued(index: Int, oldText: String, text: String, opened: OpenedQueue) {
        val before = state.value
        if (!sameQueue(before.queue, opened) || opened.revision == null) return
        launchGuarded(_steering) {
            // Each step names the queue it expects, so a message another device
            // moved or dropped meanwhile is never mistaken for an equal one.
            // An edit the host accepts against a revision raises it by exactly one.
            val revision = if (text != oldText) {
                session.editQueued(index, oldText, text, opened.revision)
                opened.revision?.plus(1)
            } else {
                opened.revision
            }
            session.steerQueued(index, text, revision)
            withTimeoutOrNull(CANCEL_EVENT_TIMEOUT_MILLIS) {
                state.first {
                    !it.working ||
                        it.startedAtMillis != before.startedAtMillis ||
                        it.queue.size < before.queue.size
                }
            }
        }
    }

    /**
     * Moves queued message [index] one place [up] or down. The host checks
     * both texts, so a queue another device just changed is refused instead
     * of being shuffled wrongly.
     */
    fun moveQueued(index: Int, text: String, up: Boolean, opened: OpenedQueue) {
        val queue = state.value.queue
        // Queued messages have no identity but their place, and texts can
        // repeat, so an action applies only to the queue it was opened on,
        // which the host checks by revision.
        if (!sameQueue(queue, opened)) return
        val anchor = if (up) index - 1 else index + 1
        if (queue.getOrNull(index) != text || anchor !in queue.indices) return
        // Swapping two equal messages changes nothing, and the queue would never
        // look changed for the wait below to end.
        if (queue[anchor] == text) return
        if (opened.revision == null) return
        launchGuarded(_droppingQueued) {
            session.reorderQueued(index, text, anchor, queue[anchor], after = !up, revision = opened.revision)
            withTimeoutOrNull(QUEUE_EVENT_TIMEOUT_MILLIS) {
                state.first { it.queue != queue }
            }
        }
    }

    private val _pullRequest = MutableStateFlow<String?>(null)
    private var pullRequestReads = 0L
    private var pullRequestBranch: String? = null

    /** The pull request of the branch this chat works on, for the session header. */
    val pullRequest: StateFlow<String?> = _pullRequest.asStateFlow()

    /** Bumped when the host reports a Git change to this chat's checkout. */
    val gitChanges: StateFlow<Long> = session.gitChanges

    /**
     * Asks GitHub, through the host, whether the checked-out branch has a pull
     * request. A failure on the same branch leaves the last answer.
     */
    fun refreshPullRequest(branch: String?, fetch: Boolean = true) {
        // Another branch's pull request is never right, even if this lookup
        // fails or waits; any lookup still out was for the old branch.
        if (branch != pullRequestBranch) {
            pullRequestBranch = branch
            _pullRequest.value = null
            pullRequestReads++
        }
        if (!fetch) return
        val read = ++pullRequestReads
        viewModelScope.launch {
            try {
                // The answer names its branch; one for another branch than the
                // session's is not shown, and "GitHub could not say" keeps the
                // last link rather than erasing it.
                val lookup = session.pullRequest()
                val url = lookup?.url
                if (read == pullRequestReads) {
                    when {
                        lookup == null || lookup.branch != branch -> _pullRequest.value = null
                        url == null -> Unit
                        else -> _pullRequest.value = url.ifBlank { null }
                    }
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                // Leave whatever was shown.
            }
        }
    }

    /**
     * The Pull requests dialog: what is typed into its link field, whether a
     * link or a removal is out, and what went wrong. Kept here so a rotation
     * neither loses the paste nor the outcome of a request still out.
     */
    data class PullRequestLinks(
        val draft: String = "",
        val linking: Boolean = false,
        val removing: Set<String> = emptySet(),
        val error: String? = null,
    )

    private val _pullRequestLinks = MutableStateFlow(PullRequestLinks())
    val pullRequestLinks: StateFlow<PullRequestLinks> = _pullRequestLinks.asStateFlow()

    fun editPullRequestDraft(text: String) {
        _pullRequestLinks.update { it.copy(draft = text, error = null) }
    }

    /** Bumped when the dialog closes, so a reply that lands later does not refill the next one. */
    private var pullRequestDialogs = 0L

    /** Forgets the dialog's draft and error when it closes. A request still out keeps running. */
    fun closePullRequests() {
        pullRequestDialogs++
        _pullRequestLinks.update { it.copy(draft = "", error = null) }
    }

    /**
     * Links every pull request address in the draft. The host broadcasts the
     * new list in `tree`; what it refuses stays in the field with an error.
     */
    fun linkPullRequests() {
        val current = _pullRequestLinks.value
        if (current.linking) return
        val urls = pastedPullRequestLinks(current.draft)
        if (urls.isEmpty()) return
        if (urls.size > Ops.MAX_PULL_REQUEST_LINKS) {
            _pullRequestLinks.update { it.copy(error = "Link at most ${Ops.MAX_PULL_REQUEST_LINKS} pull requests at a time.") }
            return
        }
        _pullRequestLinks.update { it.copy(linking = true, error = null) }
        val dialog = pullRequestDialogs
        viewModelScope.launch {
            try {
                val reply = client.linkPullRequests(chatId, urls)
                _pullRequestLinks.update {
                    when {
                        dialog != pullRequestDialogs -> it.copy(linking = false)
                        reply.invalid.isEmpty() -> it.copy(draft = "", linking = false)
                        else -> it.copy(draft = reply.invalid.joinToString(" "), linking = false, error = NOT_A_PULL_REQUEST_LINK)
                    }
                }
            } catch (error: CancellationException) {
                _pullRequestLinks.update { it.copy(linking = false) }
                throw error
            } catch (error: Throwable) {
                _pullRequestLinks.update {
                    if (dialog != pullRequestDialogs) {
                        it.copy(linking = false)
                    } else {
                        it.copy(linking = false, error = error.message?.takeIf(String::isNotBlank) ?: NOT_A_PULL_REQUEST_LINK)
                    }
                }
            }
        }
    }

    /** Removes a linked pull request; the host's own lookups will not link it again. */
    fun unlinkPullRequest(url: String) {
        if (url in _pullRequestLinks.value.removing) return
        _pullRequestLinks.update { it.copy(removing = it.removing + url, error = null) }
        val dialog = pullRequestDialogs
        viewModelScope.launch {
            try {
                client.unlinkPullRequest(chatId, url)
                _pullRequestLinks.update { it.copy(removing = it.removing - url) }
            } catch (error: CancellationException) {
                _pullRequestLinks.update { it.copy(removing = it.removing - url) }
                throw error
            } catch (error: Throwable) {
                _pullRequestLinks.update {
                    it.copy(
                        removing = it.removing - url,
                        error = if (dialog == pullRequestDialogs) error.message ?: "Could not remove the pull request." else it.error,
                    )
                }
            }
        }
    }

    /** Continues a turn that was stopped, cut off, or ended by a usage limit. */
    fun resumeTurn() {
        // One stopped at its quota goes on only with a new quota.
        if (!state.value.resumable || state.value.working || state.value.quotaReached) return
        launchGuarded(_sending) { session.resumeTurn() }
    }

    private val _quotaBusy = MutableStateFlow(false)
    /** A new quota is on its way, so the quota card waits. */
    val quotaBusy: StateFlow<Boolean> = _quotaBusy.asStateFlow()
    private val _quotaError = MutableStateFlow<String?>(null)
    val quotaError: StateFlow<String?> = _quotaError.asStateFlow()

    /**
     * Continues a chat that stopped at its quota, with a new quota of
     * [percent], or without a limit when null. The card stays busy until the
     * chat reloads without its quota mark, so it is not sent twice.
     */
    fun continueWithQuota(percent: Int?) {
        if (_quotaBusy.value) return
        _quotaBusy.value = true
        _quotaError.value = null
        viewModelScope.launch {
            try {
                session.setQuota(percent?.let(Ops.ChatQuotaChoice::Percent) ?: Ops.ChatQuotaChoice.Unlimited)
                withTimeoutOrNull(QUEUE_EVENT_TIMEOUT_MILLIS) { state.first { !it.quotaReached } }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _quotaError.value = error.message ?: "Could not continue the chat"
            } finally {
                _quotaBusy.value = false
            }
        }
    }

    fun dropQueued(index: Int, text: String, opened: OpenedQueue) {
        val before = state.value.queue
        if (!sameQueue(before, opened) || before.getOrNull(index) != text) return
        if (opened.revision == null) return
        launchGuarded(_droppingQueued) {
            session.dropQueued(index, text, opened.revision)
            withTimeoutOrNull(QUEUE_EVENT_TIMEOUT_MILLIS) {
                state.first { it.queue != before }
            }
        }
    }

    fun loadOlder() {
        launchGuarded { session.loadOlder() }
    }

    /** False, with a notice saying so, when the queue moved on since a message was opened. */
    private fun sameQueue(queue: List<String>, opened: OpenedQueue): Boolean {
        // The revision too: equal texts can stand for a queue that changed.
        val revisionHeld = opened.revision == null || state.value.queueRevision == opened.revision
        if (queue == opened.messages && revisionHeld) {
            _queueNotice.value = null
            return true
        }
        _queueNotice.value = QUEUE_CHANGED
        return false
    }

    /** The queue changed while a message was open, so its dialog closed. */
    fun queueChanged() {
        _queueNotice.value = QUEUE_CHANGED
    }

    private fun launchGuarded(
        guard: MutableStateFlow<Boolean>? = null,
        block: suspend () -> Unit,
    ) {
        if (guard?.value == true) return
        guard?.value = true
        viewModelScope.launch {
            try {
                block()
            } catch (error: CancellationException) {
                throw error
            } catch (_: Throwable) {
                // ChatSession records mutation failures in state.
            } finally {
                guard?.value = false
            }
        }
    }

    override fun onCleared() {
        draftSyncJob?.cancel()
        if (draftTextDirty || draftAttachmentsDirty) {
            val text = _draft.value
            val images = if (draftAttachmentsDirty) {
                _attachments.value.map(Attachment::png)
            } else {
                null
            }
            viewModelScope.launch(
                context = NonCancellable,
                start = CoroutineStart.UNDISPATCHED,
            ) {
                runCatching { session.setDraft(text, images) }
                session.close()
            }
        } else {
            session.close()
        }
    }

    class Factory(
        private val client: XdClient,
        private val chatId: String,
    ) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T =
            ChatViewModel(client, chatId) as T
    }

    private companion object {
        const val DRAFT_SYNC_DELAY_MILLIS = 250L
        const val QUEUE_EVENT_TIMEOUT_MILLIS = 5_000L
        const val CANCEL_EVENT_TIMEOUT_MILLIS = 5_000L
    }

    private fun scheduleDraftSync(attachments: Boolean = false) {
        draftTextDirty = true
        draftAttachmentsDirty = draftAttachmentsDirty || attachments
        draftSyncJob?.cancel()
        draftSyncJob = viewModelScope.launch {
            delay(DRAFT_SYNC_DELAY_MILLIS)
            val includeAttachments = draftAttachmentsDirty
            val images = if (includeAttachments) {
                _attachments.value.map(Attachment::png)
            } else {
                null
            }
            draftTextDirty = false
            draftAttachmentsDirty = false
            try {
                session.setDraft(_draft.value, images)
            } catch (error: CancellationException) {
                throw error
            } catch (_: Throwable) {
                draftTextDirty = true
                draftAttachmentsDirty = draftAttachmentsDirty || includeAttachments
            }
        }
    }

    private fun sameImages(
        current: List<Attachment>,
        synced: List<com.restartfu.xd.protocol.PngAttachment>,
    ): Boolean = current.size == synced.size && current.indices.all { index ->
        current[index].png.bytes.contentEquals(synced[index].bytes)
    }
}

private const val QUEUE_CHANGED = "The queue changed on another device. Open the message again."

/** A sign-in code being typed, and whether it has been sent and not yet answered. */
data class SignInDraft(val text: String = "", val sending: Boolean = false, val attempt: String = "")

/**
 * Tells one sign-in attempt from the next: each is for one account and has
 * its own sign-in page address or device code, so a code typed for one never
 * reaches another.
 */
internal fun AgentAuthProviderReply.signInAttempt(): String =
    signingIn()?.let { "${it.id}\n${it.loginUrl.orEmpty()}\n${it.deviceCode.orEmpty()}" }.orEmpty()
