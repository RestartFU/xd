package com.restartfu.xd.model

public enum class DirectAgent(
    public val wire: String,
    public val label: String,
) {
    CODEX("codex", "Codex"),
    CLAUDE("claude", "Claude"),
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
)

public fun TreeSnapshot.minimalProjects(): List<MinimalProject> = folders.map { folder ->
    val sessions = chats.filter { chat ->
        chat.folderId == folder.id && DirectAgent.fromBackend(chat.backend) != null
    }
    MinimalProject(
        id = folder.id,
        name = folder.name,
        sessions = sessions.size,
        working = sessions.count { it.working || it.terminalWorking },
    )
}

public fun TreeSnapshot.minimalSessions(projectId: String): List<MinimalSession> = chats
    .filter { it.folderId == projectId }
    .mapNotNull { chat ->
        val agent = DirectAgent.fromBackend(chat.backend) ?: return@mapNotNull null
        MinimalSession(
            id = chat.id,
            title = chat.title.takeIf(String::isNotBlank) ?: "New Session",
            agent = agent,
            branch = chat.branch?.takeIf(String::isNotBlank) ?: "Project directory",
            working = chat.working || chat.terminalWorking,
        )
    }
