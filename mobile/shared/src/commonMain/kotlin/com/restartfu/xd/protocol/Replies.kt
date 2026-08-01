package com.restartfu.xd.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

@Serializable
public data class PairReply(
    val ok: Boolean,
    val token: String,
)

@Serializable
public data class HelloReply(
    val ok: Boolean,
    val device: String,
    val version: Int,
)

@Serializable
public data class FolderReply(
    val id: String,
    val name: String,
    val parent: String? = null,
)

@Serializable
public data class ChatSummaryReply(
    val id: String,
    val folder: String,
    val title: String,
    val backend: String,
    val working: Boolean,
)

@Serializable
public data class TreeReply(
    val ok: Boolean,
    val folders: List<FolderReply>,
    val chats: List<ChatSummaryReply>,
)

@Serializable
public data class WorktreeReply(
    val path: String,
    val branch: String? = null,
    val detached: Boolean,
    val main: Boolean,
    val current: Boolean,
)

@Serializable
public data class LiveItemReply(
    val tool: Boolean,
    val text: String,
)

@Serializable
public data class ChatReply(
    val ok: Boolean,
    val title: String,
    val backend: String,
    val commands: List<String> = emptyList(),
    val plan: Boolean,
    val queued: String? = null,
    val queue: List<String> = emptyList(),
    val working: Boolean,
    val label: String? = null,
    // Present only while a turn is live. Together they mark exactly how much
    // of that turn this snapshot already contains, so the covered `text` and
    // `tool` events can be dropped instead of applied twice.
    @SerialName("turn_id")
    val turnId: Long? = null,
    @SerialName("turn_sequence")
    val turnSequence: Long? = null,
    @SerialName("working_for")
    val workingFor: Long? = null,
    val items: List<LiveItemReply> = emptyList(),
    val segment: String? = null,
    val model: String? = null,
    val effort: String? = null,
    val access: String? = null,
    @SerialName("context_used")
    val contextUsed: Long? = null,
    @SerialName("context_window")
    val contextWindow: Long? = null,
    @SerialName("new_worktree")
    val newWorktree: Boolean,
    @SerialName("has_messages")
    val hasMessages: Boolean,
    val workdir: String? = null,
    @SerialName("linked_worktree")
    val linkedWorktree: Boolean? = null,
    val worktrees: List<WorktreeReply> = emptyList(),
)

@Serializable
public data class MessageReply(
    val role: String,
    val content: String,
    val at: Long,
    val label: String? = null,
)

@Serializable
public data class MessagesReply(
    val ok: Boolean,
    @SerialName("total_messages")
    val totalMessages: Int,
    @SerialName("last_message_id")
    val lastMessageId: Long,
    val messages: List<MessageReply>,
)

@Serializable
public data class DoneReply(
    val ok: Boolean,
    val id: String? = null,
)
