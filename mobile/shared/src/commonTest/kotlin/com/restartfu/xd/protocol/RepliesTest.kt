package com.restartfu.xd.protocol

import kotlinx.serialization.json.jsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class RepliesTest {
    @Test
    fun sessionMembershipIsAdditiveToLegacyTreeReplies() {
        val legacy = WireJson.parseToJsonElement(
            """{"ok":true,"folders":[],"chats":[{"id":"one","folder":"p","title":"One","backend":"codex","working":false}]}""",
        ).jsonObject.decodeReply<TreeReply>()
        assertEquals(emptyList(), legacy.sessions)
        assertNull(legacy.chats.single().session)

        val grouped = WireJson.parseToJsonElement(
            """{"ok":true,"folders":[],"sessions":[{"id":"s","folder":"p","title":"Workspace","chats":["one"],"terminal_chat":"session:s"}],"chats":[{"id":"one","session":"s","folder":"p","title":"One","backend":"codex","working":false}]}""",
        ).jsonObject.decodeReply<TreeReply>()
        assertEquals("s", grouped.chats.single().session)
        assertEquals("session:s", grouped.sessions.single().terminalChat)
    }

    @Test
    fun workflowStatusCarriesRunAndJobClocks() {
        val reply = WireJson.parseToJsonElement(
            """
            {"ok":true,"name":"nightly","state":"completed","conclusion":"success",
             "started_at":1754215200,"completed_at":1754215445,
             "jobs":[{"id":"101","name":"linux","state":"completed",
                      "conclusion":"success","started_at":1754215205,
                      "completed_at":1754215325},
                     {"id":"102","name":"macos","state":"in_progress",
                      "started_at":1754215205}]}
            """.trimIndent(),
        ).jsonObject.decodeReply<WorkflowStatusReply>()

        assertEquals(1754215200L, reply.startedAt)
        assertEquals(1754215445L, reply.completedAt)
        assertEquals(1754215205L, reply.jobs[0].startedAt)
        assertEquals(1754215325L, reply.jobs[0].completedAt)
        // A job still going reports no finish time rather than a zero one.
        assertNull(reply.jobs[1].completedAt)
    }

    @Test
    fun workflowStatusToleratesRepliesWithoutClocks() {
        val reply = WireJson.parseToJsonElement(
            """
            {"ok":true,"name":"nightly","state":"in_progress",
             "jobs":[{"id":"101","name":"linux","state":"in_progress"}]}
            """.trimIndent(),
        ).jsonObject.decodeReply<WorkflowStatusReply>()

        assertNull(reply.startedAt)
        assertNull(reply.jobs[0].startedAt)
    }
}
