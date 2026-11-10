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
    /** Background agents and shells its agent process is still running. */
    val backgroundTasks: Int = 0,
    /** Stopped by pausing every chat, and not resumed or started again since. */
    val paused: Boolean = false,
    /** The pull requests linked to it, oldest first; see [TreeSnapshot.linkedPullRequests]. */
    val pullRequests: List<LinkedPullRequest> = emptyList(),
    /** The chat whose agent started this one through xd's start_agent tool. */
    val parentId: String? = null,
    /** Its last reply ends on a question for whoever reads it. */
    val questionWaiting: Boolean = false,
) {
    /**
     * How many background tasks the chat is waiting on: none while a turn
     * runs, since then it is working.
     */
    val waitingOn: Int get() = if (working) 0 else backgroundTasks
}

/** A pull request linked to a chat, as `tree` lists it. */
public data class LinkedPullRequest(
    /** The canonical `https://host/owner/repo/pull/N` address. */
    val url: String,
    /** `open`, `closed` or `merged`; null until the host has asked GitHub. */
    val state: String? = null,
) {
    /** The pull request's number, or null for an address that does not end in one. */
    val number: Long? get() = pullRequestNumber(url)

    /** `owner/repo`, or null for an address without them. */
    val repository: String? get() = pullRequestRepository(url)

    val merged: Boolean get() = state == "merged"
    val closed: Boolean get() = state == "closed"
}

/** The number a pull request address ends in, such as 7 for `.../pull/7`. */
public fun pullRequestNumber(url: String): Long? =
    url.trimEnd('/').substringAfterLast('/').toLongOrNull()

/** `owner/repo` of a `https://host/owner/repo/pull/N` address. */
public fun pullRequestRepository(url: String): String? {
    val path = url.substringAfter("://", "").substringAfter('/', "").trimEnd('/').split('/')
    if (path.size < 4 || path[2] != "pull") return null
    return "${path[0]}/${path[1]}".takeIf { path[0].isNotEmpty() && path[1].isNotEmpty() }
}

/**
 * The addresses in what was pasted into a link field: one per word, so a
 * list copied from anywhere links at once. Repeats are dropped.
 */
public fun pastedPullRequestLinks(text: String): List<String> =
    text.split(Regex("[\\s,]+")).map(String::trim).filter(String::isNotEmpty).distinct()

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
    /** The session whose agent started this one; its row nests under that one's. */
    val parentId: String? = null,
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
    /** The connected host can pause every chat at once and resume them. */
    val pauseAll: Boolean = false,
    /** The connected host can pause and resume one project's chats. */
    val pauseFolders: Boolean = false,
    /**
     * The connected host keeps pull requests linked to chats, so each chat's
     * [ChatSummary.pullRequests] is the whole list and can be changed.
     */
    val linkedPullRequests: Boolean = false,
) {
    /**
     * New folders can be made. A host that already lists one keeps them even
     * without saying so, as v0.2.4 hosts do; an older host would quietly make
     * a project instead.
     */
    val canCreateFolders: Boolean get() = folderGroups || folders.any(Folder::group)
}

/** What a project's pause button does when pressed. */
public enum class PauseAllAction { PAUSE, RESUME }

/**
 * The pause button of project [folderId]: resume while any of its chats is
 * paused, pause while any is at work, and null otherwise, or on a host that
 * cannot pause one project, so the button hides. Chats in folders or projects
 * inside it count too, as the host counts them.
 */
public fun TreeSnapshot.pauseActionFor(folderId: String): PauseAllAction? {
    if (!pauseFolders) return null
    val parents = folders.associate { it.id to it.parentId }
    fun within(id: String): Boolean {
        var current: String? = id
        // Bounded, so a cycle in a broken tree cannot spin forever.
        repeat(folders.size + 1) {
            val here = current ?: return false
            if (here == folderId) return true
            current = parents[here]
        }
        return false
    }
    val scoped = chats.filter { within(it.folderId) }
    return when {
        scoped.any(ChatSummary::paused) -> PauseAllAction.RESUME
        scoped.any { it.working || it.backgroundTasks > 0 } -> PauseAllAction.PAUSE
        else -> null
    }
}
