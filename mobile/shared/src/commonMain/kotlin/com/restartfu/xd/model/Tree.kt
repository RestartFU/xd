package com.restartfu.xd.model

public data class Folder(
    val id: String,
    val name: String,
    val parentId: String?,
    /** A folder of projects rather than a project. */
    val group: Boolean = false,
    /** The GitHub account behind it, whose avatar a client shows. */
    val owner: String? = null,
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
    val settledAt: Long? = null,
    val autoSettleDisabled: Boolean = false,
)

public data class TreeSnapshot(
    val folders: List<Folder> = emptyList(),
    val chats: List<ChatSummary> = emptyList(),
    val loading: Boolean = false,
    val error: String? = null,
    val sessions: List<WorkspaceSession> = emptyList(),
    /** The connected host can settle sessions. */
    val settlement: Boolean = false,
    /** The connected host says it keeps folders of projects. */
    val folderGroups: Boolean = false,
) {
    /**
     * New folders can be made. A host that already lists one keeps them even
     * without saying so, as v0.2.4 hosts do; an older host would quietly make
     * a project instead.
     */
    val canCreateFolders: Boolean get() = folderGroups || folders.any(Folder::group)
}
