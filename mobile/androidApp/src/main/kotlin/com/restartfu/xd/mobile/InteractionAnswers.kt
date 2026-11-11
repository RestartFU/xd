package com.restartfu.xd.mobile

import com.restartfu.xd.XdClient
import com.restartfu.xd.model.PendingInteraction
import java.util.WeakHashMap
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * The reader's answers to the agents' questions on one host, for every chat.
 *
 * Kept beside the client rather than in a chat's screen, which goes when the
 * reader leaves the chat: an answer on its way, and the words of a refused
 * one, outlive that. A typed answer is sent like a message, so the composer
 * empties at once; its words wait here until the host replies, and come back
 * to the chat's composer, when that is empty, only if the host refused them.
 */
class InteractionAnswers internal constructor() {
    // Answers are sent from here, not from a chat's screen, so leaving the
    // chat does not cancel one on its way.
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /** Answers kept so far: chat, then request, then question. */
    private val answers = MutableStateFlow(emptyMap<String, Map<String, Map<String, String>>>())

    /** Requests whose answers are on their way, by chat. */
    private val submitting = MutableStateFlow(emptyMap<String, Set<String>>())

    /** Words of refused answers, by chat, waiting for an empty composer. */
    private val givenBack = MutableStateFlow(emptyMap<String, String>())

    fun answers(chatId: String): Flow<Map<String, Map<String, String>>> =
        answers.map { it[chatId].orEmpty() }.distinctUntilChanged()

    fun answersNow(chatId: String): Map<String, Map<String, String>> = answers.value[chatId].orEmpty()

    fun submitting(chatId: String): Flow<Set<String>> =
        submitting.map { it[chatId].orEmpty() }.distinctUntilChanged()

    fun submittingNow(chatId: String): Set<String> = submitting.value[chatId].orEmpty()

    fun givenBack(chatId: String): Flow<String?> = givenBack.map { it[chatId] }.distinctUntilChanged()

    /**
     * Answers one of [request]'s questions. The last answer sends them all,
     * as the agent asked for them together. [typed] is the words as the
     * reader typed them, given back if the host refuses the answers.
     */
    fun answer(
        client: XdClient,
        chatId: String,
        request: PendingInteraction,
        questionId: String,
        answer: String,
        typed: String? = null,
    ) {
        val requestId = request.requestId
        if (answer.isBlank() || requestId in submittingNow(chatId)) return
        val given = answersNow(chatId)[requestId].orEmpty() + (questionId to answer)
        answers.update { it + (chatId to (it[chatId].orEmpty() + (requestId to given))) }
        if (!request.questions.all { it.id in given }) return
        submitting.update { it + (chatId to (it[chatId].orEmpty() + requestId)) }
        scope.launch {
            val sent = runCatching { client.openChat(chatId).use { it.answer(requestId, given) } }
            submitting.update { all ->
                val left = all[chatId].orEmpty() - requestId
                if (left.isEmpty()) all - chatId else all + (chatId to left)
            }
            if (sent.isFailure) {
                // Not kept, so its last question asks again, if the request
                // is still there; the answers before it stay.
                answers.update { all ->
                    val chat = all[chatId] ?: return@update all
                    val kept = chat[requestId] ?: return@update all
                    all + (chatId to (chat + (requestId to (kept - questionId))))
                }
                typed?.takeIf { it.isNotBlank() }?.let { giveBack(chatId, it) }
            }
        }
    }

    /** Forgets a request the host resolved or cleared: nothing will ask it again. */
    fun resolved(chatId: String, requestId: String) {
        answers.update { all ->
            val chat = all[chatId] ?: return@update all
            if (requestId !in chat) return@update all
            val left = chat - requestId
            if (left.isEmpty()) all - chatId else all + (chatId to left)
        }
    }

    /** Takes the given-back [words] for the chat, if they are still the ones waiting. */
    fun takeGivenBack(chatId: String, words: String): Boolean {
        var taken = false
        givenBack.update { all ->
            if (all[chatId] == words) {
                taken = true
                all - chatId
            } else {
                all
            }
        }
        return taken
    }

    private fun giveBack(chatId: String, words: String) {
        givenBack.update { all -> all + (chatId to (all[chatId]?.let { "$it\n\n$words" } ?: words)) }
    }

    companion object {
        private val stores = WeakHashMap<XdClient, InteractionAnswers>()

        /** The answers kept for [client]'s host, held no longer than the client is. */
        fun of(client: XdClient): InteractionAnswers =
            synchronized(stores) { stores.getOrPut(client) { InteractionAnswers() } }
    }
}
