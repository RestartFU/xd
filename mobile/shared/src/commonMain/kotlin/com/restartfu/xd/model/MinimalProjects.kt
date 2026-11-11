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
    /** The GitHub account behind its repository, whose avatar stands in for its icon. */
    val owner: String? = null,
    val parentId: String? = null,
)

public data class MinimalSession(
    val id: String,
    val title: String,
    /** Null for an assistant this client does not run, such as the retired JCode. */
    val agent: DirectAgent?,
    val branch: String,
    val working: Boolean,
    val chatIds: List<String> = listOf(id),
    /** Background tasks the session waits on while none of its chats has a turn running. */
    val backgroundTasks: Int = 0,
    val terminalChatId: String = id,
    /** Seconds since the epoch when the session was settled; null while it is active. */
    val settledAt: Long? = null,
    /** Kept out of the host's automatic settling. */
    val autoSettleDisabled: Boolean = false,
    /** The host's backend id, kept when [agent] is null so the chat still lists. */
    val backend: String = agent?.wire.orEmpty(),
    /** One of its chats was stopped by pausing every chat and has not resumed. */
    val paused: Boolean = false,
    /** The session whose agent started this one through xd's start_agent tool. */
    val parentId: String? = null,
    /**
     * How many sessions up the line of agents that started one another this
     * one sits, as [nestedUnderParents] lists it: 0 when no listed session did.
     */
    val depth: Int = 0,
) {
    val settled: Boolean get() = settledAt != null

    /**
     * The host groups chats into sessions. Older hosts show each chat as its
     * own session under the chat's id, so session operations do not apply.
     */
    val grouped: Boolean get() = terminalChatId.startsWith("session:")
}

/** What a session or chat waiting on its background tasks says, as on the desktop. */
public fun backgroundTasksLabel(count: Int): String =
    "Waiting on $count background task${if (count == 1) "" else "s"}"

/**
 * How many background tasks the row below the chat names: between turns, the
 * background agents and shells still running, unless the agents tray already
 * lists what runs, which the row would only repeat. As on the desktop.
 */
public fun backgroundRowCount(working: Boolean, trayShown: Boolean, waitingOn: Int): Int =
    if (working || trayShown) 0 else waitingOn

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

/**
 * Legacy hosts expose each chat as a session; newer hosts group independent tabs.
 * A chat on an assistant this client does not know, such as the retired JCode,
 * still lists so its history stays reachable; the host refuses new messages to it.
 */
public fun TreeSnapshot.minimalSessions(projectId: String): List<MinimalSession> {
    val directChats = chats.filter {
        it.folderId == projectId && it.backend.isNotBlank()
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
            agent = DirectAgent.fromBackend(first.backend),
            branch = (workspace?.branch ?: first.branch)?.takeIf(String::isNotBlank)
                ?: "Project directory",
            working = grouped.any { it.working || it.terminalWorking },
            chatIds = orderedIds,
            backgroundTasks = if (grouped.any { it.working }) 0 else grouped.sumOf { it.waitingOn },
            terminalChatId = workspace?.terminalChatId ?: first.id,
            settledAt = workspace?.settledAt,
            autoSettleDisabled = workspace?.autoSettleDisabled ?: false,
            backend = first.backend,
            paused = grouped.any(ChatSummary::paused),
            parentId = workspace?.parentId,
        )
    }
}

/**
 * A project's sessions that are not settled, in the host's order, except that
 * each one an agent started lists right below the session that started it.
 */
public fun TreeSnapshot.activeSessions(projectId: String): List<MinimalSession> =
    minimalSessions(projectId).filterNot(MinimalSession::settled).nestedUnderParents()

/**
 * Lists each session an agent started right below the session that started
 * it, a level deeper, in the order they came, and so on down. One whose
 * parent is not listed, such as a settled one, stays where it was.
 */
public fun List<MinimalSession>.nestedUnderParents(): List<MinimalSession> {
    val listed = map(MinimalSession::id).toSet()
    fun parentOf(session: MinimalSession): String? =
        session.parentId?.takeIf { it != session.id && it in listed }
    val placed = mutableSetOf<String>()
    val nested = ArrayList<MinimalSession>(size)
    // Depth first, each row's children in their own order. A row is placed
    // once, so a loop of parents cannot repeat one.
    val pending = ArrayDeque(filter { parentOf(it) == null }.map { it to 0 })
    while (pending.isNotEmpty()) {
        val (session, depth) = pending.removeFirst()
        if (!placed.add(session.id)) continue
        nested += session.copy(depth = depth)
        val children = filter { parentOf(it) == session.id && it.id !in placed }
        children.asReversed().forEach { pending.addFirst(it to depth + 1) }
    }
    // Sessions whose parents start one another in a loop still show.
    return nested + filter { it.id !in placed }
}

/** Every settled session, most recently settled first, as the desktop's shelf lists them. */
public fun TreeSnapshot.settledSessions(): List<SettledSession> = minimalProjects().flatMap { project ->
    minimalSessions(project.id).filter(MinimalSession::settled).map { SettledSession(project, it) }
}.sortedByDescending { it.session.settledAt }
