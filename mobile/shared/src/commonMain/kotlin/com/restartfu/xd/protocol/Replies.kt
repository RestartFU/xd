package com.restartfu.xd.protocol

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray

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
    val branch: String? = null,
    @SerialName("terminal_working")
    val terminalWorking: Boolean = false,
)

@Serializable
public data class TreeReply(
    val ok: Boolean,
    val folders: List<FolderReply>,
    val chats: List<ChatSummaryReply>,
)

@Serializable
public data class ShortcutsReply(
    val ok: Boolean,
    val global: List<String>,
    val workspace: List<String>,
    val effective: List<String>,
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
public data class DraftAttachmentReply(
    val name: String = "image.png",
    val mime: String,
    val data: String,
)

@Serializable
public data class ChatReply(
    val ok: Boolean,
    val title: String,
    val backend: String,
    val commands: List<String> = emptyList(),
    val shortcuts: List<String> = emptyList(),
    val plan: Boolean,
    val fast: Boolean = false,
    val queued: String? = null,
    val queue: List<String> = emptyList(),
    val draft: String = "",
    @SerialName("draft_revision")
    val draftRevision: Long = 0,
    @SerialName("draft_attachments")
    val draftAttachments: List<DraftAttachmentReply> = emptyList(),
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
    @SerialName("selected_worktree")
    val selectedWorktree: String? = null,
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

@Serializable
public data class WorkflowJobReply(
    val id: String,
    val name: String,
    val state: String,
    val conclusion: String? = null,
    val log: String? = null,
    @SerialName("started_at")
    val startedAt: Long? = null,
    @SerialName("completed_at")
    val completedAt: Long? = null,
)

@Serializable
public data class WorkflowStatusReply(
    val ok: Boolean,
    val name: String = "",
    val state: String = "",
    val conclusion: String? = null,
    val jobs: List<WorkflowJobReply> = emptyList(),
    @SerialName("started_at")
    val startedAt: Long? = null,
    @SerialName("completed_at")
    val completedAt: Long? = null,
)

@Serializable
public data class ModelReply(
    val id: String,
    val name: String,
    @SerialName("context_window")
    val contextWindow: Long = 0,
)

@Serializable
public data class BackendReply(
    val id: String,
    val name: String,
    @SerialName("default_model")
    val defaultModel: String = "",
    val models: List<ModelReply> = emptyList(),
    val efforts: List<String> = emptyList(),
)

@Serializable
public data class AgentCatalogReply(
    val ok: Boolean,
    val backends: List<BackendReply> = emptyList(),
)

@Serializable
public data class HostUpdateReply(
    val ok: Boolean,
    val version: String = "",
    val channel: String = "",
    val state: String = "idle",
    val supported: Boolean = false,
    val available: Boolean = false,
    val latest: String? = null,
    val error: String? = null,
)

@Serializable
public data class ImageReply(
    val ok: Boolean,
    val mime: String = "",
    val data: String = "",
)

@Serializable
public data class DiffReply(
    val ok: Boolean,
    val output: String = "",
)

@Serializable
public data class FileEntryReply(
    val name: String,
    val directory: Boolean,
)

@Serializable
public data class BrowseListReply(
    val ok: Boolean,
    val entries: List<FileEntryReply> = emptyList(),
)

@Serializable
public data class DirectoryListReply(
    val ok: Boolean,
    val path: String = "",
    val entries: List<String> = emptyList(),
)

@Serializable
public data class BrowseReadReply(
    val ok: Boolean,
    val content: String = "",
)

/**
 * [replay] is the host's bounded output history: `{"data": base64}` frames
 * with `{"columns","rows"}` resize frames between them. It is kept as raw JSON
 * because the order of the two kinds is the point -- a late device has to
 * interpret older output at the geometry that produced it.
 */
@Serializable
public data class TerminalReply(
    val id: String,
    val title: String = "",
    val agent: String? = null,
    val columns: Int,
    val rows: Int,
    val replay: JsonArray = JsonArray(emptyList()),
)

@Serializable
public data class TerminalListReply(
    val ok: Boolean,
    val terminals: List<TerminalReply> = emptyList(),
)

@Serializable
public data class TerminalOpenReply(
    val ok: Boolean,
    val id: String,
)
