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
    /**
     * Its chats but the agents started in its own worktree: only a wait on the
     * user passes from those to the row.
     */
    val ownChatIds: List<String> = chatIds,
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
    /** One of its chats stopped at its quota and waits for the user to give it a new one. */
    val quotaReached: Boolean = false,
    /** The session whose agent started this one through xd's start_agent tool. */
    val parentId: String? = null,
    /**
     * An agent started this session with xd's start_agent tool, in a worktree
     * of its own. It lists only while it has something going on, and its
     * project does not count it; see [finishedAgent].
     */
    val startedByAgent: Boolean = false,
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
 * lists at least that many running in the transcript, so the row would only
 * repeat it. Tasks the transcript does not show, such as hidden ones or those
 * in pages not loaded yet, still bring the row back. As on the desktop.
 */
public fun backgroundRowCount(working: Boolean, listed: Int, waitingOn: Int): Int =
    if (working || listed >= waitingOn) 0 else waitingOn

/** A settled session on the shelf, with the project it belongs to. */
public data class SettledSession(
    val project: MinimalProject,
    val session: MinimalSession,
)

/**
 * The projects, without the folders that hold them. Projects count only
 * active sessions: settled ones sit on the shelf instead, and the ones agents
 * started list under their leads, which already wait on them.
 */
public fun TreeSnapshot.minimalProjects(): List<MinimalProject> = folders.filterNot(Folder::group).map { folder ->
    val sessions = minimalSessions(folder.id).filterNot { it.settled || it.startedByAgent }
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
        val ids = orderedIds.toSet()
        // An agent started in the session's own worktree is one of its tabs,
        // but the row only waits on it, as on the agents in worktrees of their own.
        val own = grouped.filterNot { it.parentId != null && it.parentId in ids }
        val agentsAtWork = chats.count { it.parentId in ids && (it.working || it.backgroundTasks > 0) }
        MinimalSession(
            id = id,
            title = (workspace?.title ?: first.title).takeIf(String::isNotBlank) ?: "New Session",
            agent = DirectAgent.fromBackend(first.backend),
            branch = (workspace?.branch ?: first.branch)?.takeIf(String::isNotBlank)
                ?: "Project directory",
            working = own.any { it.working } || grouped.any { it.terminalWorking },
            chatIds = orderedIds,
            ownChatIds = orderedIds.filter { id -> own.any { it.id == id } },
            backgroundTasks = if (own.any { it.working }) 0 else own.sumOf { it.waitingOn } + agentsAtWork,
            terminalChatId = workspace?.terminalChatId ?: first.id,
            settledAt = workspace?.settledAt,
            autoSettleDisabled = workspace?.autoSettleDisabled ?: false,
            backend = first.backend,
            paused = own.any(ChatSummary::paused),
            quotaReached = grouped.any(ChatSummary::quotaReached),
            parentId = workspace?.parentId,
            startedByAgent = if (workspace != null) workspace.parentId != null else first.parentId != null,
        )
    }
}

/**
 * An agent's session leaves the list once it is done: nothing runs, and it is
 * neither paused nor stopped at its quota. Its lead got its report, and its
 * card in the lead's chat still opens it. [needsUser] tells a session that
 * waits on the user's answer, which stays.
 */
public fun MinimalSession.finishedAgent(needsUser: Boolean): Boolean =
    startedByAgent && !working && backgroundTasks == 0 && !paused && !quotaReached && !needsUser

/**
 * [activeSessions] as the projects list shows them: without the agents'
 * sessions that are done; see [finishedAgent].
 */
public fun TreeSnapshot.listedSessions(
    projectId: String,
    needsUser: (MinimalSession) -> Boolean,
): List<MinimalSession> = activeSessions(projectId).filterNot { it.finishedAgent(needsUser(it)) }

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

/**
 * How many sessions [nestedUnderParents] lists right below the one at [index]
 * because its agent started them, or theirs did, and so on down. Folding that
 * session hides exactly these.
 */
public fun List<MinimalSession>.startedBelow(index: Int): Int {
    val depth = this[index].depth
    return drop(index + 1).takeWhile { it.depth > depth }.size
}

/**
 * The sessions above [sessionId] in the line of agents that started one
 * another, nearest first: the ones to unfold so its row shows.
 */
public fun TreeSnapshot.sessionsAbove(sessionId: String): List<String> {
    val above = mutableListOf<String>()
    var parent = sessions.firstOrNull { it.id == sessionId }?.parentId
    while (parent != null && parent != sessionId && parent !in above) {
        above += parent
        parent = sessions.firstOrNull { it.id == parent }?.parentId
    }
    return above
}

/** Every settled session, most recently settled first, as the desktop's shelf lists them. */
public fun TreeSnapshot.settledSessions(): List<SettledSession> = minimalProjects().flatMap { project ->
    minimalSessions(project.id).filter(MinimalSession::settled).map { SettledSession(project, it) }
}.sortedByDescending { it.session.settledAt }
