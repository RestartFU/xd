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
    fun parsesLiveRecordsWithStepsAndStats() {
        val run = SubagentRun.parse(
            "subagent_run\n" +
                """{"key":"toolu_1","provider":"Claude","agent":"Explore","description":"List files",""" +
                """"status":"running","background":true,"activity":"Reading main.rs",""" +
                """"steps":[{"tool":"Bash","text":"${'$'} ls"}],"tool_uses":2,"tokens":13901,"duration_ms":2940}""",
        )!!

        assertEquals("toolu_1", run.key)
        assertEquals("Explore", run.identity)
        assertEquals("List files", run.detail)
        assertEquals(SubagentState.RUNNING, run.state)
        assertEquals(true, run.background)
        assertEquals("Reading main.rs", run.activity)
        assertEquals(listOf(SubagentStep("Bash", "${'$'} ls")), run.steps)
        assertEquals("2 tools · 13.9k tokens · 2.9s", run.stats)

        val done = SubagentRun.parse(
            "subagent_run\n" + """{"agent":"Explore","status":"completed","summary":"Two files."}""",
        )!!
        assertEquals(SubagentState.SUCCESS, done.state)
        assertEquals("Done", done.status)
        assertEquals("Two files.", done.summary)
        assertNull(SubagentRun.parse("subagent_run\nnot json"))
    }

    @Test
    fun countsAndDurationsReadCompactly() {
        assertEquals("950", compactCount(950))
        assertEquals("16.2k", compactCount(16_210))
        assertEquals("200k", compactCount(200_000))
        assertEquals("1.2M", compactCount(1_200_000))
        assertEquals("2.9s", compactDuration(2_940))
        assertEquals("47s", compactDuration(47_000))
        assertEquals("3m 05s", compactDuration(185_000))
        assertEquals("1h 02m", compactDuration(3_720_000))
    }

    @Test
    fun rejectsMalformedMarkers() {
        assertNull(SubagentRun.parse("subagent\nonly one line"))
        assertNull(SubagentRun.parse("subagent\n\nRunning · work"))
        assertNull(SubagentRun.parse("tool\nCodex\nRunning · work"))
    }
}
