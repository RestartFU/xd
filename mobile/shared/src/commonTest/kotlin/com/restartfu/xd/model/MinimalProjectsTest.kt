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
    fun thePauseButtonPausesUntilSomethingIsPausedThenResumes() {
        val idle = TreeSnapshot(chats = listOf(chat("one", "p", "Idle", "codex")), pauseAll = true)
        // An older host cannot pause every chat, so the button hides.
        assertEquals(null, idle.copy(pauseAll = false).pauseAllAction)
        assertEquals(PauseAllAction.PAUSE, idle.pauseAllAction)
        assertEquals(false, idle.anythingToPause)

        val working = idle.copy(chats = idle.chats + chat("two", "p", "Busy", "claude", working = true))
        assertEquals(PauseAllAction.PAUSE, working.pauseAllAction)
        assertEquals(true, working.anythingToPause)
        // A chat waiting on background tasks between turns is paused too.
        val background = idle.copy(chats = listOf(chat("one", "p", "Idle", "codex").copy(backgroundTasks = 1)))
        assertEquals(true, background.anythingToPause)
        // A terminal running a command is not a chat at work.
        val terminal = idle.copy(chats = listOf(chat("one", "p", "Idle", "codex", terminalWorking = true)))
        assertEquals(false, terminal.anythingToPause)

        val paused = idle.copy(chats = idle.chats + chat("two", "p", "Busy", "claude").copy(paused = true))
        assertEquals(PauseAllAction.RESUME, paused.pauseAllAction)
        assertEquals(listOf(false, true), paused.minimalSessions("p").map { it.paused })
    }
}
