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
