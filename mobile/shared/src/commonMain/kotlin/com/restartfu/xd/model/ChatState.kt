package com.restartfu.xd.model

import com.restartfu.xd.protocol.PngAttachment

public enum class TranscriptKind {
    USER,
    ASSISTANT,
    TOOL,
    SYSTEM,
}

public data class TranscriptItem(
    val id: String,
    val kind: TranscriptKind,
    val text: String,
    val atMillis: Long? = null,
    val label: String? = null,
    val live: Boolean = false,
    /** The host's id for the stored row, when a record may rewrite it. */
    val messageId: Long? = null,
)

public enum class TodoStatus {
    PENDING,
    IN_PROGRESS,
    COMPLETED,
}

public data class TodoItem(
    val id: String,
    val text: String,
    val status: TodoStatus,
)

public data class ChatState(
    val chatId: String,
    val title: String = "",
    val backend: String = "",
    /** Absent until the chat is loaded; the host then reports the selection. */
    val model: String? = null,
    val effort: String? = null,
    /** `read-only`, `edit` or `full`. Ignored by the host while [plan] is on. */
    val access: String? = null,
    /** The id of the account the chat is set to run on; null while the host chooses. */
    val account: String? = null,
    val newWorktree: Boolean = false,
    /** True when this chat is in Git and has not locked its checkout yet. */
    val canCreateWorktree: Boolean = false,
    val commands: List<String> = emptyList(),
    val shortcuts: List<String> = emptyList(),
    val plan: Boolean = false,
    val fast: Boolean = false,
    val queue: List<String> = emptyList(),
    /** The host's count of changes to [queue]; null from hosts that keep none. */
    val queueRevision: Long? = null,
    val draft: String = "",
    val draftRevision: Long = 0,
    val draftAttachments: List<PngAttachment> = emptyList(),
    val working: Boolean = false,
    /** The last turn was cut short, so the composer can offer to continue it. */
    val resumable: Boolean = false,
    /** The quota that applies to this chat, while one does. */
    val quota: ChatQuota? = null,
    /**
     * Stopped at its quota: it continues only once the user gives it a new
     * one, so the composer offers that instead of resuming.
     */
    val quotaReached: Boolean = false,
    val label: String? = null,
    /** Live turn this state reflects, and how far into it we have applied. */
    val turnEpoch: String = "",
    val turnId: Long? = null,
    val turnSequence: Long = 0,
    val startedAtMillis: Long? = null,
    val messages: List<TranscriptItem> = emptyList(),
    val liveItems: List<TranscriptItem> = emptyList(),
    val liveSegment: String = "",
    val todos: List<TodoItem> = emptyList(),
    val interactions: List<PendingInteraction> = emptyList(),
    val interactionRevision: Long = 0,
    val interactionEpoch: String = "",
    val pendingUser: TranscriptItem? = null,
    val transcriptLoaded: Boolean = false,
    /**
     * The chat's own reply has been applied, so [draft] is the host's draft
     * rather than the empty default. Messages alone do not set it.
     */
    val draftLoaded: Boolean = false,
    val loading: Boolean = false,
    val loadingOlder: Boolean = false,
    val hasOlderMessages: Boolean = false,
    /**
     * Host id of the oldest stored message fetched so far, hidden rows
     * included, so "Load older" asks for the page before it even when a
     * whole page held only hidden rows.
     */
    val oldestFetchedMessageId: Long? = null,
    /** Whether [todos] came from a snapshot, which may be an empty list. */
    val todosFromSnapshot: Boolean = false,
    val error: String? = null,
) {
    public val visibleItems: List<TranscriptItem>
        get() = buildList {
            addAll(messages)
            pendingUser?.let(::add)
            addAll(liveItems)
            segmentItem?.let(::add)
        }

    /** The reply still streaming in, drawn after everything else. */
    public val segmentItem: TranscriptItem?
        get() = liveSegment.takeIf(String::isNotEmpty)?.let {
            TranscriptItem(
                id = LIVE_SEGMENT_ID,
                kind = TranscriptKind.ASSISTANT,
                text = it,
                live = true,
            )
        }

    /** The last of [visibleItems], without building the list. */
    public val lastVisibleItem: TranscriptItem?
        get() = segmentItem ?: liveItems.lastOrNull() ?: pendingUser ?: messages.lastOrNull()

    public companion object {
        public const val LIVE_SEGMENT_ID: String = "live-segment"
    }
}

public data class PendingInteraction(
    val requestId: String,
    val method: String,
    val detail: String,
    val questions: List<PendingQuestion> = emptyList(),
    /** The agent's tool asking, such as Bash or Edit, when it says. */
    val tool: String? = null,
    /** Why it asks, when that is more than the command itself. */
    val reason: String? = null,
    /** The edit it wants to make, as a unified patch. */
    val patch: String? = null,
)
public data class PendingQuestion(
    val id: String,
    val text: String,
    val options: List<String>,
    val secret: Boolean = false,
)
