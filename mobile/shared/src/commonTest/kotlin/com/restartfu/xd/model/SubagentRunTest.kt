package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class SubagentRunTest {
    @Test
    fun parsesKeyedCodexAgents() {
        val run = SubagentRun.parse(
            "subagent\nthread-1\nCodex · gpt-5.6-sol · high\n" +
                "Running · Review the diff · Agent thread-1",
        )!!

        assertEquals("thread-1", run.key)
        assertEquals("Codex · gpt-5.6-sol · high", run.identity)
        assertEquals("Running", run.status)
        assertEquals(SubagentState.RUNNING, run.state)
        assertEquals("Review the diff · Agent thread-1", run.detail)
    }

    @Test
    fun parsesLegacyClaudeAgentsAndTerminalStates() {
        val legacy = SubagentRun.parse(
            "subagent\nClaude · Explore agent\nInspect the parser · Trace every path",
        )!!
        assertNull(legacy.key)
        assertEquals("Running", legacy.status)
        assertEquals("Inspect the parser · Trace every path", legacy.detail)

        val completed = SubagentRun.parse(
            "subagent\nthread-2\nCodex\nCompleted · Tests passed",
        )!!
        assertEquals(SubagentState.SUCCESS, completed.state)
        assertEquals("Completed", completed.status)
        assertEquals("Tests passed", completed.detail)
    }

    @Test
    fun rejectsMalformedMarkers() {
        assertNull(SubagentRun.parse("subagent\nonly one line"))
        assertNull(SubagentRun.parse("subagent\n\nRunning · work"))
        assertNull(SubagentRun.parse("tool\nCodex\nRunning · work"))
    }
}
