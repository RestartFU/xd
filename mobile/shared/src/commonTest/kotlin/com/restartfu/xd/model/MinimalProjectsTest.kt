package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals

class MinimalProjectsTest {
    @Test
    fun projectsSummarizeEverySessionInWorkspaceOrder() {
        val snapshot = TreeSnapshot(
            folders = listOf(
                Folder("compiler", "Compiler", null),
                Folder("website", "Website", null),
            ),
            chats = listOf(
                chat("one", "compiler", "Fix parser", "codex", working = true),
                chat("two", "compiler", "Review lexer", "claude"),
                chat("old", "compiler", "Legacy chat", "jcode"),
                chat(
                    "three",
                    "website",
                    "Polish home",
                    "codex",
                    terminalWorking = true,
                ),
            ),
        )

        assertEquals(
            listOf(
                MinimalProject("compiler", "Compiler", sessions = 3, working = 1),
                MinimalProject("website", "Website", sessions = 1, working = 1),
            ),
            snapshot.minimalProjects(),
        )
    }

    @Test
    fun projectSessionsKeepAgentBranchAndFallbackTitle() {
        val snapshot = TreeSnapshot(
            chats = listOf(
                chat("one", "compiler", "Fix parser", "codex", branch = "fix/parser"),
                chat("two", "compiler", "", "claude"),
                chat("three", "website", "Elsewhere", "codex"),
                chat("old", "compiler", "Legacy chat", "jcode"),
            ),
        )

        assertEquals(
            listOf(
                MinimalSession(
                    id = "one",
                    title = "Fix parser",
                    agent = DirectAgent.CODEX,
                    branch = "fix/parser",
                    working = false,
                ),
                MinimalSession(
                    id = "two",
                    title = "New Session",
                    agent = DirectAgent.CLAUDE,
                    branch = "Project directory",
                    working = false,
                ),
                // A retired assistant's chat still lists, with no agent of its own.
                MinimalSession(
                    id = "old",
                    title = "Legacy chat",
                    agent = null,
                    branch = "Project directory",
                    working = false,
                    backend = "jcode",
                ),
            ),
            snapshot.minimalSessions("compiler"),
        )
    }

    @Test
    fun siblingChatsShareOneSessionWithoutSharingAgentIdentity() {
        val snapshot = TreeSnapshot(
            folders = listOf(Folder("project", "Compiler", null)),
            chats = listOf(
                chat("first", "project", "Implement", "codex").copy(sessionId = "session"),
                chat("second", "project", "Review", "claude", working = true).copy(sessionId = "session"),
            ),
            sessions = listOf(WorkspaceSession("session", "project", "Parser work", listOf("second", "first"), branch = "fix/parser")),
        )
        val session = snapshot.minimalSessions("project").single()
        assertEquals("session", session.id)
        assertEquals("Parser work", session.title)
        assertEquals(listOf("second", "first"), session.chatIds)
        assertEquals("session:session", session.terminalChatId)
        assertEquals("fix/parser", session.branch)
        assertEquals(true, session.working)
        assertEquals(MinimalProject("project", "Compiler", 1, 1), snapshot.minimalProjects().single())
        assertEquals(listOf("codex", "claude"), snapshot.chats.map { it.backend })
    }

    @Test
    fun aSessionWaitsOnItsBackgroundTasksOnlyWhileNoTurnRuns() {
        val idle = chat("first", "project", "Implement", "claude").copy(sessionId = "session", backgroundTasks = 2)
        val sibling = chat("second", "project", "Review", "claude").copy(sessionId = "session", backgroundTasks = 1)
        val snapshot = TreeSnapshot(
            folders = listOf(Folder("project", "Compiler", null)),
            chats = listOf(idle, sibling),
            sessions = listOf(WorkspaceSession("session", "project", "Parser work", listOf("first", "second"))),
        )
        assertEquals(2, idle.waitingOn)
        assertEquals(3, snapshot.minimalSessions("project").single().backgroundTasks)

        val working = snapshot.copy(chats = listOf(idle.copy(working = true), sibling))
        assertEquals(0, working.chats.first().waitingOn, "a running turn is work")
        val session = working.minimalSessions("project").single()
        assertEquals(true, session.working)
        assertEquals(0, session.backgroundTasks)

        assertEquals("Waiting on 1 background task", backgroundTasksLabel(1))
        assertEquals("Waiting on 2 background tasks", backgroundTasksLabel(2))
    }

    @Test
    fun deletingOriginalChatKeepsSessionAndSharedTerminalIdentity() {
        val snapshot = TreeSnapshot(
            chats = listOf(chat("sibling", "project", "Review", "claude").copy(sessionId = "session")),
            sessions = listOf(WorkspaceSession("session", "project", "Parser work", listOf("sibling"))),
        )
        val session = snapshot.minimalSessions("project").single()
        assertEquals("session", session.id)
        assertEquals("Parser work", session.title)
        assertEquals(listOf("sibling"), session.chatIds)
        assertEquals("session:session", session.terminalChatId)
    }

    private fun chat(
        id: String,
        folder: String,
        title: String,
        backend: String,
        working: Boolean = false,
        branch: String? = null,
        terminalWorking: Boolean = false,
    ): ChatSummary = ChatSummary(
        id = id,
        folderId = folder,
        title = title,
        backend = backend,
        working = working,
        branch = branch,
        terminalWorking = terminalWorking,
    )

    @Test
    fun settledSessionsLeaveTheProjectForTheShelfNewestFirst() {
        val snapshot = TreeSnapshot(
            folders = listOf(Folder("compiler", "Compiler", null), Folder("website", "Website", null)),
            sessions = listOf(
                WorkspaceSession("active", "compiler", "Active", listOf("one")),
                WorkspaceSession("older", "compiler", "Older", listOf("two"), settledAt = 100),
                WorkspaceSession("newer", "website", "Newer", listOf("three"), settledAt = 200, autoSettleDisabled = true),
            ),
            chats = listOf(
                chat("one", "compiler", "One", "codex").copy(sessionId = "active"),
                chat("two", "compiler", "Two", "claude").copy(sessionId = "older"),
                chat("three", "website", "Three", "codex", working = true).copy(sessionId = "newer"),
            ),
            settlement = true,
        )

        assertEquals(listOf("active"), snapshot.activeSessions("compiler").map(MinimalSession::id))
        assertEquals(emptyList(), snapshot.activeSessions("website"))
        assertEquals(
            listOf(
                MinimalProject("compiler", "Compiler", sessions = 1, working = 0),
                MinimalProject("website", "Website", sessions = 0, working = 0),
            ),
            snapshot.minimalProjects(),
        )
        val shelf = snapshot.settledSessions()
        assertEquals(listOf("newer", "older"), shelf.map { it.session.id })
        assertEquals(listOf("Website", "Compiler"), shelf.map { it.project.name })
        assertEquals(true, shelf.first().session.autoSettleDisabled)
        assertEquals(false, shelf.last().session.autoSettleDisabled)
    }

    @Test
    fun aProjectPauseButtonFollowsItsOwnChats() {
        val folders = listOf(
            Folder("site", "Site", null),
            Folder("inner", "Inner", "site"),
            Folder("app", "App", null),
        )
        val idle = TreeSnapshot(
            folders = folders,
            chats = listOf(chat("one", "site", "Idle", "codex"), chat("two", "app", "Other", "codex")),
            pauseAll = true,
            pauseFolders = true,
        )
        // Nothing to pause hides the button, as does a host that cannot pause one project.
        assertEquals(null, idle.pauseActionFor("site"))
        val working = idle.copy(chats = idle.chats + chat("three", "inner", "Busy", "claude", working = true))
        assertEquals(null, working.copy(pauseFolders = false).pauseActionFor("site"))
        // A project inside counts toward the one around it.
        assertEquals(PauseAllAction.PAUSE, working.pauseActionFor("site"))
        assertEquals(PauseAllAction.PAUSE, working.pauseActionFor("inner"))
        assertEquals(null, working.pauseActionFor("app"))
        // A chat waiting on background tasks between turns can be paused too.
        val background = idle.copy(chats = listOf(chat("one", "app", "Idle", "codex").copy(backgroundTasks = 1)))
        assertEquals(PauseAllAction.PAUSE, background.pauseActionFor("app"))
        // A terminal running a command is not a chat at work.
        val terminal = idle.copy(chats = listOf(chat("one", "app", "Idle", "codex", terminalWorking = true)))
        assertEquals(null, terminal.pauseActionFor("app"))

        val paused = working.copy(chats = working.chats + chat("four", "site", "Held", "claude").copy(paused = true))
        assertEquals(PauseAllAction.RESUME, paused.pauseActionFor("site"))
        assertEquals(PauseAllAction.PAUSE, paused.pauseActionFor("inner"))
        assertEquals(listOf(false, true), paused.minimalSessions("site").map { it.paused })
        // A chat stopped at its quota marks its session, which is not paused.
        val stopped = working.copy(chats = working.chats + chat("five", "site", "Spent", "claude").copy(quotaReached = true))
        assertEquals(listOf(false, true), stopped.minimalSessions("site").map { it.quotaReached })
        assertEquals(listOf(false, false), stopped.minimalSessions("site").map { it.paused })
    }

    @Test
    fun sessionsAnAgentStartedListRightBelowTheSessionThatStartedThem() {
        // A tab added to a worker's session can start a worker of its own.
        val ids = listOf("w2", "other", "deep", "w1", "lead", "orphan", "settledWorker", "gone", "loopA", "loopB")
        val parents = mapOf(
            "w2" to "lead", "w1" to "lead", "deep" to "w1", "orphan" to "missing", "settledWorker" to "gone",
            "loopA" to "loopB", "loopB" to "loopA",
        )
        val snapshot = TreeSnapshot(
            chats = ids.map { chat(it, "compiler", it, "claude") },
            sessions = ids.map {
                WorkspaceSession(
                    it,
                    "compiler",
                    it,
                    listOf(it),
                    parentId = parents[it],
                    settledAt = if (it == "gone") 100 else null,
                )
            },
        )

        assertEquals(
            listOf(
                "other" to 0, "lead" to 0, "w2" to 1, "w1" to 1, "deep" to 2, "orphan" to 0, "settledWorker" to 0,
                "loopA" to 0, "loopB" to 0,
            ),
            snapshot.activeSessions("compiler").map { it.id to it.depth },
        )
        assertEquals("lead", snapshot.minimalSessions("compiler").first { it.id == "w1" }.parentId)
    }
}
