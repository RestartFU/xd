package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals

class MinimalProjectsTest {
    @Test
    fun projectsSummarizeOnlyDirectAgentSessionsInWorkspaceOrder() {
        val snapshot = TreeSnapshot(
            folders = listOf(
                Folder("compiler", "Compiler", null),
                Folder("website", "Website", null),
            ),
            chats = listOf(
                chat("one", "compiler", "Fix parser", "codex", working = true),
                chat("two", "compiler", "Review lexer", "claude"),
                chat("old", "compiler", "Legacy chat", "other"),
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
                MinimalProject("compiler", "Compiler", sessions = 2, working = 1),
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
                chat("old", "compiler", "Legacy chat", "other"),
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
}
