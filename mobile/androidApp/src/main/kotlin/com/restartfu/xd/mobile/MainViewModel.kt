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
import kotlinx.coroutines.launch

class MainViewModel(application: Application) : AndroidViewModel(application) {
    val client: XdClient = (application as XdApplication).client
    private val _pairing = MutableStateFlow(false)
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
            } catch (error: Throwable) {
                _error.value = error.message ?: "Could not pair with the daemon"
            } finally {
                _pairing.value = false
            }
        }
    }

    fun forget() {
        _error.value = null
        viewModelScope.launch {
            runCatching { client.forget() }
                .onFailure { _error.value = it.message ?: "Could not forget the remote" }
        }
    }

    fun createChat(
        folderId: String,
        onCreated: (String) -> Unit,
    ) {
        viewModelScope.launch {
            runCatching { client.createChat(folderId = folderId, title = null) }
                .onSuccess(onCreated)
                .onFailure { _error.value = it.message ?: "Could not create a chat" }
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
    val sending: StateFlow<Boolean> = _sending.asStateFlow()

    fun send(
        text: String,
        onSent: () -> Unit,
    ) {
        if (_sending.value) return
        _sending.value = true
        viewModelScope.launch {
            try {
                session.send(text)
                onSent()
            } catch (error: CancellationException) {
                throw error
            } catch (_: Throwable) {
                // ChatSession exposes the failure through its state.
            } finally {
                _sending.value = false
            }
        }
    }

    fun cancel() {
        viewModelScope.launch { runCatching { session.cancel() } }
    }

    fun dropQueued(index: Int) {
        viewModelScope.launch { runCatching { session.dropQueued(index) } }
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
}
