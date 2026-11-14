package com.restartfu.xd.mobile

import android.os.Bundle
import androidx.compose.runtime.Composable
import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.createSavedStateHandle
import androidx.lifecycle.viewModelScope
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import com.restartfu.xd.XdClient
import com.restartfu.xd.net.Link
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Composer drafts the host has not taken yet, by chat: typed while offline,
 * or in the moment before the app went to the background. Each is kept in the
 * activity's saved state through this view model's SavedStateHandle, as
 * [UnsavedFileStore] keeps file edits, so it outlives Android ending the app
 * and comes back still unsent. Every one is sent again when the connection
 * comes back up, and at once when the app goes to the background, before the
 * socket closes.
 *
 * Only the text is kept: pictures attached to a draft are too large for saved
 * state. Android holds saved state in a buffer the whole app shares, so this
 * keeps at most [limitBytes] across every chat; a draft past that is not kept
 * (it is still sent while the app runs, but lost if Android ends the app first).
 *
 * How a draft meets the host's is unchanged: the last device to write one wins.
 */
internal class UnsentDrafts @JvmOverloads constructor(
    handle: SavedStateHandle,
    private val client: XdClient,
    private val limitBytes: Long = UNSENT_DRAFT_LIMIT_BYTES,
) : ViewModel() {
    private val kept = LinkedHashMap<String, String>()

    /** The chats on screen, each sending its own draft (and its pictures) when asked. */
    private val live = mutableMapOf<String, suspend () -> Unit>()
    private var generation: Long? = null
    private var backgroundJob: Job? = null

    init {
        handle.get<Bundle>(KEY)?.let { saved ->
            for (chat in saved.keySet()) saved.getString(chat)?.let { kept[chat] = it }
        }
        // Written only when Android saves the activity, not on every keystroke.
        handle.setSavedStateProvider(KEY) {
            Bundle().apply { kept.forEach { (chat, text) -> putString(chat, text) } }
        }
        viewModelScope.launch {
            var wasUp = false
            client.link.collect { link ->
                val up = link is Link.Up
                // Back up: whatever could not be sent meanwhile goes now.
                if (up && !wasUp) launch { flush() }
                wasUp = up
            }
        }
    }

    /** The draft kept for [chatId] that the host has not taken, if any. */
    fun kept(chatId: String): String? = kept[chatId]

    /**
     * Keeps [text] as [chatId]'s draft until the host takes it. One that does
     * not fit beside the others is not kept, nor is its older text; it says
     * whether it was.
     */
    fun keep(chatId: String, text: String): Boolean {
        var others = 0L
        for ((chat, draft) in kept) if (chat != chatId) others += bytesOf(chat, draft)
        return if (others + bytesOf(chatId, text) <= limitBytes) {
            kept[chatId] = text
            true
        } else {
            kept.remove(chatId)
            false
        }
    }

    /** The host took [text] for [chatId]: no longer kept, unless it was changed since. */
    fun sent(chatId: String, text: String) {
        if (kept[chatId] == text) kept.remove(chatId)
    }

    /** [chatId] is on screen and sends its own draft through [push]. */
    fun attach(chatId: String, push: suspend () -> Unit) {
        live[chatId] = push
    }

    fun detach(chatId: String, push: suspend () -> Unit) {
        if (live[chatId] === push) live.remove(chatId)
    }

    /**
     * Sends every draft the host has not taken, now, and returns once each has
     * been answered. One that fails stays kept for the next time.
     */
    suspend fun flush() = coroutineScope {
        for (push in live.values.toList()) launch { push() }
        for ((chat, text) in kept.filterKeys { it !in live }) {
            launch {
                try {
                    client.setDraft(chat, text)
                    sent(chat, text)
                } catch (error: CancellationException) {
                    throw error
                } catch (_: Throwable) {
                    // Still kept: sent again when the connection is next up.
                }
            }
        }
    }

    /**
     * The app went to the background: drafts go to the host first, without
     * waiting out the pause after typing, then the connection closes. Coming
     * back before that ([comeForward]) keeps it open.
     */
    fun goBackground() {
        backgroundJob?.cancel()
        backgroundJob = viewModelScope.launch {
            withTimeoutOrNull(BACKGROUND_FLUSH_MILLIS) { flush() }
            client.goBackground()
        }
    }

    fun comeForward() {
        backgroundJob?.cancel()
        backgroundJob = null
    }

    /** Lets go of the drafts of chats that are gone. */
    fun retain(chatIds: Set<String>) {
        kept.keys.retainAll(chatIds)
    }

    /**
     * Follows the host's session generation: forgetting the host lets its
     * drafts go. Not the first binding, which is the app starting again with
     * drafts to bring back.
     */
    fun bindGeneration(value: Long) {
        if (generation != null && generation != value) kept.clear()
        generation = value
    }

    override fun onCleared() {
        // Ended while still sending drafts: the connection closes all the same.
        if (backgroundJob?.isActive == true) client.goBackground()
    }

    companion object {
        private const val KEY = "unsent-drafts"

        fun factory(client: XdClient): ViewModelProvider.Factory = viewModelFactory {
            initializer { UnsentDrafts(createSavedStateHandle(), client) }
        }
    }
}

/** The activity's [UnsentDrafts], for [client]. */
@Composable
internal fun rememberUnsentDrafts(client: XdClient): UnsentDrafts = viewModel(factory = UnsentDrafts.factory(client))

/** How much unsent draft text is kept through the end of the process, across every chat. */
internal const val UNSENT_DRAFT_LIMIT_BYTES = 128L * 1024

/** How long going to the background waits for drafts to reach the host before closing the connection. */
internal const val BACKGROUND_FLUSH_MILLIS = 2_000L

/** What a draft takes in saved state: its chat id and text, two bytes a character. */
private fun bytesOf(chatId: String, text: String): Long = 2L * (chatId.length + text.length)
