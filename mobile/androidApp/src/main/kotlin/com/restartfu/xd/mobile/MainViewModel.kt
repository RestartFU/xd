package com.restartfu.xd.mobile

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.restartfu.xd.XdClient
import com.restartfu.xd.net.PairResult
import com.restartfu.xd.store.ChatSession
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
    private val _error = MutableStateFlow<String?>(null)

    val pairing: StateFlow<Boolean> = _pairing.asStateFlow()
    val error: StateFlow<String?> = _error.asStateFlow()

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

    fun createChat(
        folderId: String,
        onCreated: (String) -> Unit,
    ) {
        if (_creatingChat.value) return
        _creatingChat.value = true
        _error.value = null
        viewModelScope.launch {
            try {
                onCreated(client.createChat(folderId = folderId, title = null))
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not create a chat"
            } finally {
                _creatingChat.value = false
            }
        }
    }
}

class ChatViewModel(
    client: XdClient,
    chatId: String,
) : ViewModel() {
    val session: ChatSession = client.openChat(chatId)
    val state = session.state
    private val _sending = MutableStateFlow(false)
    private val _droppingQueued = MutableStateFlow(false)
    val sending: StateFlow<Boolean> = _sending.asStateFlow()

    fun send(
        text: String,
        onSent: () -> Unit,
    ) {
        launchGuarded(_sending) {
                session.send(text)
                onSent()
        }
    }

    fun cancel() {
        launchGuarded { session.cancel() }
    }

    fun enqueue(
        text: String,
        onQueued: () -> Unit,
    ) {
        launchGuarded(_sending) {
                session.enqueue(text)
                onQueued()
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
    }
}
