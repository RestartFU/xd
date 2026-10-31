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
)

public data class MinimalSession(
    val id: String,
    val title: String,
    val agent: DirectAgent,
    val branch: String,
    val working: Boolean,
    val chatIds: List<String> = listOf(id),
    val terminalChatId: String = id,
)

public fun TreeSnapshot.minimalProjects(): List<MinimalProject> = folders.map { folder ->
    val sessions = minimalSessions(folder.id)
    MinimalProject(
        id = folder.id,
        name = folder.name,
        sessions = sessions.size,
        working = sessions.count { it.working },
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
        )
    }
}
