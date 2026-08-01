package com.restartfu.xd.mobile

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.restartfu.xd.XdClient
import com.restartfu.xd.net.PairResult
import com.restartfu.xd.protocol.BackendReply
import com.restartfu.xd.protocol.ChatOption
import com.restartfu.xd.protocol.DaemonUpdateReply
import com.restartfu.xd.protocol.Limits
import com.restartfu.xd.store.ChatSession
import com.restartfu.xd.voice.VoiceSession
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

class MainViewModel(application: Application) : AndroidViewModel(application) {
    val client: XdClient = (application as XdApplication).client
    private val _pairing = MutableStateFlow(false)
    private val _forgetting = MutableStateFlow(false)
    private val _creatingChat = MutableStateFlow(false)
    private val _createdChat = MutableStateFlow<String?>(null)
    private val _deletingChat = MutableStateFlow(false)
    private val _error = MutableStateFlow<String?>(null)

    val pairing: StateFlow<Boolean> = _pairing.asStateFlow()
    val createdChat: StateFlow<String?> = _createdChat.asStateFlow()
    val error: StateFlow<String?> = _error.asStateFlow()
    val deletingChat: StateFlow<Boolean> = _deletingChat.asStateFlow()
    private val _daemon = MutableStateFlow<DaemonUpdateReply?>(null)
    private val _daemonError = MutableStateFlow<String?>(null)
    private val _updating = MutableStateFlow(false)
    val daemon: StateFlow<DaemonUpdateReply?> = _daemon.asStateFlow()
    val daemonError: StateFlow<String?> = _daemonError.asStateFlow()
    val daemonBusy: StateFlow<Boolean> = _updating.asStateFlow()

    fun pair(
        host: String,
        port: Int,
        code: String,
        deviceName: String,
    ) {
        if (_pairing.value) return
        _pairing.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                when (val result = client.pair(host, port, code, deviceName)) {
                    is PairResult.Success -> Unit
                    is PairResult.Failure -> _error.value = result.message
                }
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not pair with the daemon"
            } finally {
                _pairing.value = false
            }
        }
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

    /**
     * Drives the daemon update from the tree screen.
     *
     * Install and restart are separate calls because they differ in cost:
     * replacing files is safe while turns run, restarting drops every attached
     * device and loses the turn.
     */
    fun daemonUpdate(action: String = "status") {
        if (_updating.value) return
        _updating.value = true
        viewModelScope.launch {
            try {
                _daemon.value = client.daemonUpdate(action)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _daemonError.value = error.message ?: "Could not reach the daemon"
            } finally {
                _updating.value = false
            }
        }
    }

    fun clearDaemonUpdate() {
        _daemon.value = null
        _daemonError.value = null
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

    /**
     * Dictation into the composer.
     *
     * The transcript is appended rather than replacing the draft, so speaking
     * after typing adds to what is there — the same thing the desktop does.
     */
    val voice: VoiceSession = VoiceSession(
        transport = session,
        recorders = ::AndroidVoiceRecorder,
        scope = viewModelScope,
        onTranscript = ::appendToDraft,
    )

    init {
        viewModelScope.launch { client.voiceEvents.collect(voice::onEvent) }
    }

    fun updateDraft(value: String) {
        _draft.value = value
    }

    private fun appendToDraft(transcript: String) {
        val existing = _draft.value
        val separator = if (existing.isEmpty() || existing.last().isWhitespace()) "" else " "
        _draft.value = existing + separator + transcript
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
            if (loaded.isNotEmpty()) _attachments.value = _attachments.value + loaded
        }
    }

    fun removeAttachment(index: Int) {
        _attachments.value = _attachments.value.filterIndexed { at, _ -> at != index }
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
            if (_draft.value == text) _draft.value = ""
            _attachments.value = _attachments.value.drop(images.size)
        }
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
            if (_draft.value == text) _draft.value = ""
        }
    }

    /**
     * Loads the catalog once per chat screen.
     *
     * It is daemon state rather than app state: hard-coding the models would
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
     * The daemon overrides access while planning, so the access choice is not
     * meaningful until this is off again.
     */
    fun setPlan(planning: Boolean) {
        launchGuarded(_selectingModel) {
            session.setBoolOption(ChatOption.PLAN, planning)
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
     * Both happen in one coroutine because the daemon matches the steer
     * against what is actually queued: sending them concurrently would race,
     * and the steer would be refused for not matching an edit that had not
     * landed yet.
     *
     * The daemon promotes the message and cancels the turn, so this waits for
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
        // Leaving the chat abandons a recording rather than transcribing into
        // a composer nobody is looking at.
        voice.cancel()
        session.close()
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
        const val QUEUE_EVENT_TIMEOUT_MILLIS = 5_000L
        const val CANCEL_EVENT_TIMEOUT_MILLIS = 5_000L
    }
}
