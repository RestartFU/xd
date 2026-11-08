package com.restartfu.xd.model

public enum class DirectAgent(
    public val wire: String,
    public val label: String,
) {
    CODEX("codex", "Codex"),
    CLAUDE("claude", "Claude"),
    JCODE("jcode", "JCode"),
    ;

    public companion object {
        public fun fromBackend(backend: String): DirectAgent? =
            entries.firstOrNull { it.wire.equals(backend, ignoreCase = true) }
    }
}

public data class MinimalProject(
    val id: String,
    val name: String,
    val sessions: Int,
    val working: Int,
    /** The GitHub account behind its repository, whose avatar stands in for its icon. */
    val owner: String? = null,
    val parentId: String? = null,
)

public data class MinimalSession(
    val id: String,
    val title: String,
    val agent: DirectAgent,
    val branch: String,
    val working: Boolean,
    val chatIds: List<String> = listOf(id),
    val terminalChatId: String = id,
    /** Seconds since the epoch when the session was settled; null while it is active. */
    val settledAt: Long? = null,
    /** Kept out of the host's automatic settling. */
    val autoSettleDisabled: Boolean = false,
) {
    val settled: Boolean get() = settledAt != null

    /**
     * The host groups chats into sessions. Older hosts show each chat as its
     * own session under the chat's id, so session operations do not apply.
     */
    val grouped: Boolean get() = terminalChatId.startsWith("session:")
}

/** A settled session on the shelf, with the project it belongs to. */
public data class SettledSession(
    val project: MinimalProject,
    val session: MinimalSession,
)

/**
 * The projects, without the folders that hold them. Projects count only
 * active sessions: settled ones sit on the shelf instead.
 */
public fun TreeSnapshot.minimalProjects(): List<MinimalProject> = folders.filterNot(Folder::group).map { folder ->
    val sessions = minimalSessions(folder.id).filterNot(MinimalSession::settled)
    MinimalProject(
        id = folder.id,
        name = folder.name,
        sessions = sessions.size,
        working = sessions.count { it.working },
        owner = folder.owner,
        parentId = folder.parentId,
    )
}

/** Legacy hosts expose each chat as a session; newer hosts group independent tabs. */
public fun TreeSnapshot.minimalSessions(projectId: String): List<MinimalSession> {
    val directChats = chats.filter {
        it.folderId == projectId && DirectAgent.fromBackend(it.backend) != null
    }
    return directChats.groupBy(ChatSummary::sessionId).map { (id, grouped) ->
        val workspace = sessions.firstOrNull { it.id == id }
        val first = grouped.first()
        val orderedIds = workspace?.chatIds.orEmpty().filter { candidate ->
            grouped.any { it.id == candidate }
        }.let { ordered -> ordered + grouped.map(ChatSummary::id).filterNot { it in ordered } }
        MinimalSession(
            id = id,
            title = (workspace?.title ?: first.title).takeIf(String::isNotBlank) ?: "New Session",
            agent = checkNotNull(DirectAgent.fromBackend(first.backend)),
            branch = (workspace?.branch ?: first.branch)?.takeIf(String::isNotBlank)
                ?: "Project directory",
            working = grouped.any { it.working || it.terminalWorking },
            chatIds = orderedIds,
            terminalChatId = workspace?.terminalChatId ?: first.id,
            settledAt = workspace?.settledAt,
            autoSettleDisabled = workspace?.autoSettleDisabled ?: false,
        )
    }
}

/** A project's sessions that are not settled, in the host's order. */
public fun TreeSnapshot.activeSessions(projectId: String): List<MinimalSession> =
    minimalSessions(projectId).filterNot(MinimalSession::settled)

/** Every settled session, most recently settled first, as the desktop's shelf lists them. */
public fun TreeSnapshot.settledSessions(): List<SettledSession> = minimalProjects().flatMap { project ->
    minimalSessions(project.id).filter(MinimalSession::settled).map { SettledSession(project, it) }
}.sortedByDescending { it.session.settledAt }
