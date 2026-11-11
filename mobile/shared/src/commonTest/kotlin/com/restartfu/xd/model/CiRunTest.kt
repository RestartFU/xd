package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class CiRunTest {
    @Test
    fun aCiCardListsItsChecksAndHowTheyStand() {
        val run = CiRun.parse(
            "ci_run\n" + """{"url":"https://github.com/o/r/pull/300","number":300,""" +
                """"title":"Fix the settings test","head":"9f2c7b1","status":"running","started_at":1000,""" +
                """"checks":[{"name":"linux","workflow":"pr","state":"pending"},""" +
                """{"name":"macos","workflow":"pr","state":"passed"},""" +
                """{"name":"lint","state":"failed","conclusion":"timed out"}]}""",
        )!!
        assertEquals(CiState.RUNNING, run.state)
        assertEquals(300L, run.number)
        assertEquals("https://github.com/o/r/pull/300/checks", run.checksUrl)
        assertEquals(
            listOf(
                CiCheck("pr / linux", CiState.RUNNING, "Running"),
                CiCheck("pr / macos", CiState.PASSED, "Passed"),
                CiCheck("lint", CiState.FAILED, "Timed out"),
            ),
            run.checks,
        )
        assertEquals("1 of 3 checks running", run.summary)
        // Rows of the transcript stand on their own, never in a run of tools.
        val item = TranscriptItem("1", TranscriptKind.TOOL, "ci_run\n" + """{"url":"https://github.com/o/r/pull/7","status":"passed","checks":[{"name":"a","state":"passed"}]}""")
        assertEquals(ToolShape.Ci(CiRun.parse(item.text)!!), ToolGrouping.classify(item))
    }

    @Test
    fun aStoppedCardSaysWhyAndAFinishedOneCounts() {
        val stopped = CiRun.parse(
            "ci_run\n" + """{"url":"https://github.com/o/r/pull/7","status":"stopped","reason":"Watched again below.","checks":[]}""",
        )!!
        assertEquals(CiState.STOPPED, stopped.state)
        assertEquals("Watched again below.", stopped.summary)
        val failed = CiRun.parse(
            "ci_run\n" + """{"url":"https://github.com/o/r/pull/7","status":"failed","checks":[""" +
                """{"name":"a","state":"failed","conclusion":"failure"},{"name":"b","state":"passed"}]}""",
        )!!
        assertEquals("1 failed, 1 passed", failed.summary)
        val waiting = CiRun.parse(
            "ci_run\n" + """{"url":"https://github.com/o/r/pull/7","status":"running","waiting_for":"abc1234"}""",
        )!!
        assertEquals("Waiting for GitHub to show abc1234.", waiting.summary)
        assertNull(CiRun.parse("ci_run\nnot json"))
        assertNull(CiRun.parse("ci_run\n" + """{"url":"http://x","status":"running"}"""))
    }
}
