package com.restartfu.xd.mobile

import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.restartfu.xd.store.ChatSession
import com.restartfu.xd.terminal.Cell
import com.restartfu.xd.terminal.ReplayFrame
import com.restartfu.xd.terminal.TerminalEvent
import com.restartfu.xd.terminal.TerminalKey
import com.restartfu.xd.terminal.TerminalKeys
import com.restartfu.xd.terminal.TerminalScreen
import com.restartfu.xd.terminal.TerminalWire
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

public data class TerminalPane(
    val id: String? = null,
    val rows: List<List<Cell>> = emptyList(),
    val cursorRow: Int = 0,
    val cursorColumn: Int = 0,
    val connecting: Boolean = false,
    val closed: Boolean = false,
    val error: String? = null,
)

/**
 * One shared pty, drawn from the bytes the daemon broadcasts.
 *
 * The session lives on the daemon and every attached device sees the same
 * screen, so opening attaches to an existing terminal rather than starting a
 * second one. Replay is applied before live output, which is why the emulator
 * is fed from this one place and in order.
 */
class TerminalViewModel(
    private val session: ChatSession,
    private val chatId: String,
) : ViewModel() {
    private val screen = TerminalScreen(COLUMNS, ROWS)
    private val _state = MutableStateFlow(TerminalPane())
    val state: StateFlow<TerminalPane> = _state.asStateFlow()

    init {
        open()
    }

    fun open() {
        _state.value = TerminalPane(connecting = true)
        viewModelScope.launch {
            try {
                val existing = session.terminals().firstOrNull()
                val id = existing?.id
                    ?: session.openTerminal(COLUMNS, ROWS, reuse = true)
                // A terminal opened elsewhere already has scrollback. Replay
                // rebuilds it, resize frames included and in order, so older
                // output is interpreted at the geometry that produced it.
                existing?.let { reply ->
                    TerminalWire.frames(reply).forEach { frame ->
                        when (frame) {
                            is ReplayFrame.Output -> screen.write(frame.data)
                            is ReplayFrame.Resize -> screen.resize(frame.columns, frame.rows)
                        }
                    }
                    screen.resize(COLUMNS, ROWS)
                }
                _state.value = TerminalPane(
                    id = id,
                    rows = screen.snapshot(),
                    cursorRow = screen.cursorRow,
                    cursorColumn = screen.cursorColumn,
                )
                session.resizeTerminal(id, COLUMNS, ROWS)
            } catch (error: CancellationException) {
                throw error
            } catch (error: Throwable) {
                _state.value = TerminalPane(
                    error = error.message ?: "Could not open a terminal",
                )
            }
        }
    }

    fun onEvent(event: TerminalEvent) {
        if (event.chatId != chatId || event.terminalId != _state.value.id) return
        when (event) {
            is TerminalEvent.Output -> {
                screen.write(event.data)
                _state.value = _state.value.copy(
                    rows = screen.snapshot(),
                    cursorRow = screen.cursorRow,
                    cursorColumn = screen.cursorColumn,
                )
            }
            is TerminalEvent.Closed -> _state.value = _state.value.copy(closed = true)
        }
    }

    /**
     * Writes straight to the pty, as a terminal does.
     *
     * There is no submit step: the shell sees each keystroke as it happens,
     * which is what makes tab completion, Ctrl-C and a live prompt work at all.
     */
    fun send(bytes: ByteArray) {
        val id = _state.value.id ?: return
        viewModelScope.launch {
            runCatching { session.sendTerminalInput(id, TerminalWire.encode(bytes)) }
        }
    }

    fun type(text: String): Unit = send(TerminalKeys.text(text))

    fun press(key: TerminalKey): Unit = send(TerminalKeys.bytes(key))

    fun control(letter: Char) {
        TerminalKeys.control(letter)?.let(::send)
    }

    fun kill() {
        val id = _state.value.id ?: return
        viewModelScope.launch { runCatching { session.killTerminal(id) } }
    }

    class Factory(
        private val session: ChatSession,
        private val chatId: String,
    ) : ViewModelProvider.Factory {
        @Suppress("UNCHECKED_CAST")
        override fun <T : ViewModel> create(modelClass: Class<T>): T =
            TerminalViewModel(session, chatId) as T
    }

    private companion object {
        const val COLUMNS = 80
        const val ROWS = 24
    }
}
