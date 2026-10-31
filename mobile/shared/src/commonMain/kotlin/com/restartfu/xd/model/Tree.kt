package com.restartfu.xd.model

public data class Folder(
    val id: String,
    val name: String,
    val parentId: String?,
)

public data class ChatSummary(
    val id: String,
    val folderId: String,
    val title: String,
    val backend: String,
    val working: Boolean,
    val branch: String? = null,
    val terminalWorking: Boolean = false,
    val sessionMode: String = "chat",
    val sessionId: String = id,
)

public data class WorkspaceSession(
    val id: String,
    val folderId: String,
    val title: String,
    val chatIds: List<String>,
    val branch: String? = null,
    val working: Boolean = false,
    val terminalChatId: String = "session:$id",
)

public data class TreeSnapshot(
    val folders: List<Folder> = emptyList(),
    val chats: List<ChatSummary> = emptyList(),
    val loading: Boolean = false,
    val error: String? = null,
    val sessions: List<WorkspaceSession> = emptyList(),
)
