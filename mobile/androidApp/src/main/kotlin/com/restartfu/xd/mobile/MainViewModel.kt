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
import com.restartfu.xd.protocol.AutoSettleSettingsReply
import com.restartfu.xd.protocol.BackendReply
import com.restartfu.xd.protocol.ChatOption
import com.restartfu.xd.protocol.HostUpdateReply
import com.restartfu.xd.protocol.Limits
import com.restartfu.xd.protocol.Ops
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

class MainViewModel(application: Application) : AndroidViewModel(application) {
    val client: XdClient = (application as XdApplication).client
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
    private val _error = MutableStateFlow<String?>(null)
    private val _shortcutEditor = MutableStateFlow<ShortcutEditorState?>(null)

    val connecting: StateFlow<Boolean> = _connecting.asStateFlow()
    val pendingHostKey: StateFlow<PendingHostKeyConfirmation?> = _pendingHostKey.asStateFlow()
    val privateKeyName: StateFlow<String?> = _privateKeyName.asStateFlow()
    val createdDirectSession: StateFlow<CreatedDirectSession?> =
        _createdDirectSession.asStateFlow()
    val error: StateFlow<String?> = _error.asStateFlow()
    val deletingChat: StateFlow<Boolean> = _deletingChat.asStateFlow()
    private val _settledNotice = MutableStateFlow<SettledNotice?>(null)
    private var settledSerial = 0L
    val settledNotice: StateFlow<SettledNotice?> = _settledNotice.asStateFlow()
    private val _autoSettleError = MutableStateFlow<String?>(null)
    private var autoSettleRequests = 0L
    /** Auto-settle requests still waiting on the host, including reads a broadcast started. */
    val autoSettleBusy: StateFlow<Int> = client.autoSettleInFlight

    /** The connected host's rules; the client keeps them current and clears them on forgetting. */
    val autoSettle: StateFlow<AutoSettleSettingsReply?> = client.autoSettleSettings
    val autoSettleError: StateFlow<String?> = _autoSettleError.asStateFlow()
    val autoSettleStale: StateFlow<Boolean> = client.autoSettleStale

    init {
        // A failure belongs to the host it came from.
        viewModelScope.launch {
            client.sessionGeneration.collect {
                autoSettleRequests++
                _autoSettleError.value = null
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
                )
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

    fun createWorkspace(name: String, repository: String? = null) {
        if (_creatingWorkspace.value) return
        _creatingWorkspace.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                client.createFolder(name = name, repository = repository)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not create the workspace"
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
        client.settleSession(sessionId)
        _settledNotice.value = SettledNotice(sessionId, title, ++settledSerial)
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

    fun renameProject(folderId: String, name: String) = treeAction("Could not rename the project") {
        client.renameFolder(folderId, name)
    }

    fun deleteProject(folderId: String) = treeAction("Could not delete the project") {
        client.trashFolder(folderId)
    }

    fun loadAutoSettle() = autoSettleRequest("Could not read the auto-settle rules") {
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
    private fun autoSettleRequest(failure: String, request: suspend () -> Unit) {
        val serial = ++autoSettleRequests
        _autoSettleError.value = null
        viewModelScope.launch {
            try {
                request()
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (serial == autoSettleRequests) _autoSettleError.value = error.message ?: failure
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
    chatId: String,
) : ViewModel() {
    val session: ChatSession = client.openChat(chatId)
    val state = session.state
    val speech = session.speech
    private val _sending = MutableStateFlow(false)
    private val _cancelling = MutableStateFlow(false)
    private val _droppingQueued = MutableStateFlow(false)
    private val _steering = MutableStateFlow(false)
    private val _draft = MutableStateFlow("")
    private val _attachments = MutableStateFlow(emptyList<Attachment>())
    private val _attachmentError = MutableStateFlow<String?>(null)
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

    suspend fun resetSpeech() {
        session.resetSpeech()
    }

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
            // on while the request was in flight.
            if (_draft.value == text) {
                _draft.value = ""
                _attachments.value = _attachments.value.drop(images.size)
                scheduleDraftSync(attachments = true)
            }
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

    fun enqueue() {
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

    fun editQueued(index: Int, oldText: String, text: String) {
        launchGuarded(_droppingQueued) {
            session.editQueued(index, oldText, text)
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
    fun steerQueued(index: Int, oldText: String, text: String) {
        val before = state.value
        launchGuarded(_steering) {
            if (text != oldText) session.editQueued(index, oldText, text)
            session.steerQueued(index, text)
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
    fun moveQueued(index: Int, text: String, up: Boolean) {
        val queue = state.value.queue
        // The queue may have changed on another device since the message was
        // opened, so it is found again by its text; the host checks both texts.
        val at = queuedAt(queue, index, text) ?: return
        val anchor = if (up) at - 1 else at + 1
        if (anchor !in queue.indices) return
        launchGuarded(_droppingQueued) {
            session.reorderQueued(at, text, anchor, queue[anchor], after = !up)
            withTimeoutOrNull(QUEUE_EVENT_TIMEOUT_MILLIS) {
                state.first { it.queue != queue }
            }
        }
    }

    /** Continues a turn that was stopped, cut off, or ended by a usage limit. */
    fun resumeTurn() {
        if (!state.value.resumable || state.value.working) return
        launchGuarded(_sending) { session.resumeTurn() }
    }

    fun dropQueued(index: Int, text: String) {
        val before = state.value.queue
        // Found again by its text if another device changed the queue since it
        // was opened; already gone means there is nothing left to drop.
        val at = queuedAt(before, index, text) ?: return
        launchGuarded(_droppingQueued) {
            session.dropQueued(at, text)
            withTimeoutOrNull(QUEUE_EVENT_TIMEOUT_MILLIS) {
                state.first { it.queue != before }
            }
        }
    }

    fun loadOlder() {
        launchGuarded { session.loadOlder() }
    }

    /** Where [text] sits now: its old [index] if unchanged, else its nearest copy. */
    private fun queuedAt(queue: List<String>, index: Int, text: String): Int? =
        if (queue.getOrNull(index) == text) index
        else queue.indices.filter { queue[it] == text }.minByOrNull { kotlin.math.abs(it - index) }

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
