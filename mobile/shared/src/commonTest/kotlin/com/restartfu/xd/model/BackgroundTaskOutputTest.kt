package com.restartfu.xd.model

import com.restartfu.xd.protocol.BackgroundTaskOutputReply
import com.restartfu.xd.protocol.Ops
import com.restartfu.xd.protocol.WireJson
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.json.jsonPrimitive

class BackgroundTaskOutputTest {
    @Test fun decodesOutputErrorsUnavailableFilesAndOldEmptyReplies() {
        val reply = WireJson.decodeFromString<BackgroundTaskOutputReply>("""{"ok":true,"chat":"c","tasks":[
            {"id":"watch","kind":"Monitor","description":"Watch CI","started_at":123,"output_file":"/tmp/watch.output",
             "lines":[{"text":"Checking"},{"text":"jq: command not found","error":true}],"truncated":true},
            {"id":"gone","output_error":"Output file unavailable."}]}""")
        assertEquals(123L, reply.tasks[0].startedAt)
        assertFalse(reply.tasks[0].lines[0].error)
        assertTrue(reply.tasks[0].lines[1].error)
        assertTrue(reply.tasks[0].truncated)
        assertEquals("Output file unavailable.", reply.tasks[1].outputError)
        assertTrue(WireJson.decodeFromString<BackgroundTaskOutputReply>("""{"ok":true,"chat":"c"}""").tasks.isEmpty())
        assertEquals("background-task-output", Ops.backgroundTaskOutput("c")["op"]?.jsonPrimitive?.content)
        assertEquals("c", Ops.backgroundTaskOutput("c")["chat"]?.jsonPrimitive?.content)
    }
}
