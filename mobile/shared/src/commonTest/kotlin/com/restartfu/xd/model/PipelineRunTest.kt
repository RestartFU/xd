package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class PipelineRunTest {
    @Test
    fun parsesCapturedWorkflowMarkers() {
        val marker = "workflow_run\n123\n" +
            "https://github.com/RestartFU/xd/actions/runs/123"

        val run = PipelineRun.parse(marker)

        requireNotNull(run)
        assertEquals("123", run.id)
        assertEquals("RestartFU/xd", run.repository)
        assertEquals(marker, run.marker)
    }

    @Test
    fun rejectsMalformedWorkflowMarkers() {
        assertNull(PipelineRun.parse("plain tool output"))
        assertNull(
            PipelineRun.parse(
                "workflow_run\nnope\n" +
                    "https://github.com/RestartFU/xd/actions/runs/nope",
            ),
        )
        assertNull(
            PipelineRun.parse(
                "workflow_run\n123\nhttps://example.com/RestartFU/xd/actions/runs/123",
            ),
        )
        assertNull(
            PipelineRun.parse(
                "workflow_run\n123\n" +
                    "https://github.com/RestartFU/xd/other/123",
            ),
        )
    }
}
