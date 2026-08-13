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
import com.restartfu.xd.net.ConnectResult
import com.restartfu.xd.model.DirectAgent
import com.restartfu.xd.protocol.BackendReply
import com.restartfu.xd.protocol.ChatOption
import com.restartfu.xd.protocol.HostUpdateReply
import com.restartfu.xd.protocol.Limits
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
)

data class CreatedDirectSession(
    val projectId: String,
    val chatId: String,
    val agent: DirectAgent,
    val title: String,
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
    private var privateKeyBytes: ByteArray? = null
    private val _forgetting = MutableStateFlow(false)
    private val _creatingChat = MutableStateFlow(false)
    private val _creatingWorkspace = MutableStateFlow(false)
    private val _moving = MutableStateFlow(false)
    private val _createdChat = MutableStateFlow<String?>(null)
    private val _createdDirectSession = MutableStateFlow<CreatedDirectSession?>(null)
    private val _deletingChat = MutableStateFlow(false)
    private val _renamingChat = MutableStateFlow(false)
    private val _error = MutableStateFlow<String?>(null)
    private val _shortcutEditor = MutableStateFlow<ShortcutEditorState?>(null)

    val connecting: StateFlow<Boolean> = _connecting.asStateFlow()
    val pendingHostKey: StateFlow<PendingHostKeyConfirmation?> = _pendingHostKey.asStateFlow()
    val privateKeyName: StateFlow<String?> = _privateKeyName.asStateFlow()
    val createdChat: StateFlow<String?> = _createdChat.asStateFlow()
    val createdDirectSession: StateFlow<CreatedDirectSession?> =
        _createdDirectSession.asStateFlow()
    val error: StateFlow<String?> = _error.asStateFlow()
    val deletingChat: StateFlow<Boolean> = _deletingChat.asStateFlow()
    val moving: StateFlow<Boolean> = _moving.asStateFlow()
    val shortcutEditor: StateFlow<ShortcutEditorState?> = _shortcutEditor.asStateFlow()
    private val _host = MutableStateFlow<HostUpdateReply?>(null)
    private val _hostError = MutableStateFlow<String?>(null)
    private val _updating = MutableStateFlow(false)
    val host: StateFlow<HostUpdateReply?> = _host.asStateFlow()
    val hostError: StateFlow<String?> = _hostError.asStateFlow()
    val hostBusy: StateFlow<Boolean> = _updating.asStateFlow()

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

    fun createChat(folderId: String) {
        if (_creatingChat.value) return
        _creatingChat.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                _createdChat.value = client.createChat(folderId = folderId, title = null)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not create a chat"
            } finally {
                _creatingChat.value = false
            }
        }
    }

    fun createDirectSession(
        folderId: String,
        title: String,
        agent: DirectAgent,
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
                )
                _createdDirectSession.value = CreatedDirectSession(
                    folderId,
                    chatId,
                    agent,
                    title.takeIf(String::isNotBlank) ?: "New Session",
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

    fun moveFolder(folderId: String, parentId: String?) {
        if (_moving.value) return
        _moving.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                client.moveFolder(folderId, parentId)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not move the folder"
            } finally {
                _moving.value = false
            }
        }
    }

    fun moveChat(chatId: String, folderId: String) {
        if (_moving.value) return
        _moving.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                client.moveChat(chatId, folderId)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not move the chat"
            } finally {
                _moving.value = false
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

    fun openShortcutEditor(folderId: String?, title: String) {
        val opened = ShortcutEditorState(folderId = folderId, title = title)
        _shortcutEditor.value = opened
        viewModelScope.launch {
            try {
                val reply = client.shortcuts(folderId)
                if (_shortcutEditor.value?.folderId == folderId) {
                    _shortcutEditor.value = opened.copy(
                        prompts = if (folderId == null) reply.global else reply.workspace,
                        loading = false,
                    )
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (_shortcutEditor.value?.folderId == folderId) {
                    _shortcutEditor.value = opened.copy(
                        loading = false,
                        error = error.message ?: "Could not load shortcuts",
                    )
                }
            }
        }
    }

    fun closeShortcutEditor() {
        _shortcutEditor.value = null
    }

    fun saveShortcuts(prompts: List<String>) {
        val editor = _shortcutEditor.value ?: return
        if (editor.saving) return
        _shortcutEditor.value = editor.copy(saving = true, error = null)
        viewModelScope.launch {
            try {
                client.setShortcuts(editor.folderId, prompts)
                if (_shortcutEditor.value?.folderId == editor.folderId) {
                    _shortcutEditor.value = null
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                if (_shortcutEditor.value?.folderId == editor.folderId) {
                    _shortcutEditor.value = editor.copy(
                        saving = false,
                        error = error.message ?: "Could not save shortcuts",
                    )
                }
            }
        }
    }

    /**
     * Drives the host update from the tree screen.
     *
     * Install and restart are separate calls because they differ in cost:
     * replacing files is safe while turns run, restarting drops every attached
     * device and loses the turn.
     */
    fun hostUpdate(action: String = "status") {
        if (_updating.value) return
        _updating.value = true
        viewModelScope.launch {
            try {
                _host.value = client.hostUpdate(action)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _hostError.value = error.message ?: "Could not reach the host"
            } finally {
                _updating.value = false
            }
        }
    }

    fun clearHostUpdate() {
        _host.value = null
        _hostError.value = null
    }

    fun consumeCreatedChat(chatId: String) {
        _createdChat.compareAndSet(chatId, null)
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
    val catalog: StateFlow<List<BackendReply>> = _catalog.asStateFlow()
    val catalogLoading: StateFlow<Boolean> = _catalogLoading.asStateFlow()
    val catalogError: StateFlow<String?> = _catalogError.asStateFlow()
    val selectingModel: StateFlow<Boolean> = _selectingModel.asStateFlow()
    val draft: StateFlow<String> = _draft.asStateFlow()
    private var draftSyncJob: Job? = null
    private var draftTextDirty = false
    private var draftAttachmentsDirty = false
    private var draftRevision = -1L

    suspend fun resetSpeech() {
        session.resetSpeech()
    }

    init {
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

    fun selectModel(backend: String, model: String) {
        launchGuarded(_selectingModel) {
            session.selectModel(backend, model)
        }
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
     * The host promotes the message and cancels the turn, so this waits for
     * the turn to actually change rather than reporting success while the old
     * one is still winding down.
     */
    fun steerQueued(index: Int, oldText: String, text: String) {
        val before = state.value
        launchGuarded(_steering) {
            if (text != oldText) session.editQueued(index, oldText, text)
            session.steerQueued(index, text)
            withTimeoutOrNull(CANCEL_EVENT_TIMEOUT_MILLIS) {
                state.first {
                    !it.working || it.startedAtMillis != before.startedAtMillis
                }
            }
        }
    }

    fun dropQueued(index: Int) {
        val before = state.value.queue
        launchGuarded(_droppingQueued) {
            session.dropQueued(index)
            withTimeoutOrNull(QUEUE_EVENT_TIMEOUT_MILLIS) {
                state.first { it.queue != before }
            }
        }
    }

    fun loadOlder() {
        launchGuarded { session.loadOlder() }
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
