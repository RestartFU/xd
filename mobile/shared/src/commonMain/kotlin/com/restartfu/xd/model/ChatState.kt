package com.restartfu.xd.model

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
)

public data class ChatState(
    val chatId: String,
    val title: String = "",
    val backend: String = "",
    val commands: List<String> = emptyList(),
    val plan: Boolean = false,
    val queue: List<String> = emptyList(),
    val working: Boolean = false,
    val label: String? = null,
    /** Live turn this state reflects, and how far into it we have applied. */
    val turnId: Long? = null,
    val turnSequence: Long = 0,
    val startedAtMillis: Long? = null,
    val messages: List<TranscriptItem> = emptyList(),
    val liveItems: List<TranscriptItem> = emptyList(),
    val liveSegment: String = "",
    val pendingUser: TranscriptItem? = null,
    val loading: Boolean = false,
    val loadingOlder: Boolean = false,
    val hasOlderMessages: Boolean = false,
    val error: String? = null,
) {
    public val visibleItems: List<TranscriptItem>
        get() = buildList {
            addAll(messages)
            pendingUser?.let(::add)
            addAll(liveItems)
            if (liveSegment.isNotEmpty()) {
                add(
                    TranscriptItem(
                        id = "live-segment",
                        kind = TranscriptKind.ASSISTANT,
                        text = liveSegment,
                        live = true,
                    ),
                )
            }
        }
}
