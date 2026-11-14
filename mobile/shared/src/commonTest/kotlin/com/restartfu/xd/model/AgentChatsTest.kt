package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class AgentChatsTest {
    @Test
    fun anAgentReportNamesItsChatTitleAndOutcome() {
        assertEquals(
            AgentReport("c-1", "Fix (the) tests", AgentOutcome.DONE),
            AgentReport.parse("[xd] Agent \"Fix (the) tests\" (c-1) is done with its turn.\n\nIts reply:\n\nOk."),
        )
        assertEquals(
            AgentReport("c-2", "", AgentOutcome.FAILED),
            AgentReport.parse("[xd] An agent (c-2) is stopped with an error."),
        )
        assertEquals(AgentOutcome.WAITING, AgentReport.parse("[xd] Agent \"Q\" (c-3) is waiting for an answer.")?.outcome)
        assertNull(AgentReport.parse("Agent \"Q\" (c-3) is done with its turn."))
        assertNull(AgentReport.parse("[xd] something else"))
    }

    @Test
    fun aCiReportNamesItsPullRequestAndOutcome() {
        assertEquals(
            CiReport("https://github.com/o/r/pull/291", "291", CiOutcome.FAILED),
            CiReport.parse("[xd] CI failed on https://github.com/o/r/pull/291 at 1e3d4a4.\n\n1 of 2 checks did not pass"),
        )
        assertEquals(CiOutcome.PASSED, CiReport.parse("[xd] CI passed on https://github.com/o/r/pull/7 at abc.")?.outcome)
        assertEquals(
            CiReport("https://github.com/o/r/pull/7", "7", CiOutcome.STOPPED),
            CiReport.parse("[xd] Stopped watching CI on https://github.com/o/r/pull/7.\n\nMerged."),
        )
        assertNull(CiReport.parse("CI passed on https://github.com/o/r/pull/7 at abc."))
        assertNull(CiReport.parse("[xd] CI passed on https://github.com/o/r/issues/7 at a."))
        assertNull(CiReport.parse("[xd] CI passed on the moon."))
        // An agent's report is not a CI report.
        assertNull(CiReport.parse("[xd] Agent \"Q\" (c-3) is done with its turn."))
    }

    @Test
    fun theTrayCountsAgentChatsAtWorkAndThoseWaitingOnTheUser() {
        val working = ChatSummary("a", "p", "A", "claude", working = true)
        val asking = ChatSummary("b", "p", "B", "claude", working = false, questionWaiting = true)
        assertEquals("1 agent working", AgentsTray.label(emptyList(), listOf(working)))
        assertEquals("2 agents working · 1 needs you", AgentsTray.label(emptyList(), listOf(working, working, asking)))
        assertEquals("2 need you", AgentsTray.label(emptyList(), listOf(asking, asking)))
        // A turn waiting on an approval needs the user, though it still runs.
        val approving = working.waitingOnUser(true)
        assertEquals("1 needs you", AgentsTray.label(emptyList(), listOf(approving)))
        assertEquals(working, working.waitingOnUser(false))
    }

    @Test
    fun theChatAsItIsNowWinsOverTheCardsLastTurn() {
        val card = SubagentRun.parse("subagent_run\n{\"key\":\"chat:c\",\"chat\":\"c\",\"agent\":\"T\",\"status\":\"running\"}")!!
        assertEquals("c", card.chat)
        val chat = ChatSummary("c", "p", "T", "claude", working = false)
        assertEquals(AgentChatStatus.RUNNING, agentChatStatus(card, chat.copy(working = true)))
        assertEquals(AgentChatStatus.PAUSED, agentChatStatus(card, chat.copy(paused = true)))
        assertEquals(AgentChatStatus.NEEDS_YOU, agentChatStatus(card, chat.copy(questionWaiting = true)))
        assertEquals(AgentChatStatus.STOPPED, agentChatStatus(card, chat))
        // A card for a chat that is gone does not run forever.
        assertEquals(AgentChatStatus.STOPPED, agentChatStatus(card, null))
        val done = card.copy(state = SubagentState.SUCCESS)
        assertEquals(AgentChatStatus.DONE, agentChatStatus(done, chat))
        assertEquals(AgentChatStatus.FAILED, agentChatStatus(card.copy(state = SubagentState.FAILURE), chat))
    }
}
