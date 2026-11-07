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

    @Test
    fun settlementMembersAreOptional() {
        val legacy = WireJson.parseToJsonElement(
            """{"ok":true,"folders":[],"chats":[],"sessions":[{"id":"s","folder":"p","title":"W","chats":[]}]}""",
        ).jsonObject.decodeReply<TreeReply>()
        assertEquals(false, legacy.settlement)
        assertNull(legacy.sessions.single().settledAt)
        assertEquals(false, legacy.sessions.single().autoSettleDisabled)

        val settled = WireJson.parseToJsonElement(
            """{"ok":true,"settlement":true,"folders":[],"chats":[],"sessions":[{"id":"s","folder":"p","title":"W","chats":[],"settled_at":1700000000,"auto_settle_disabled":true}]}""",
        ).jsonObject.decodeReply<TreeReply>()
        assertEquals(true, settled.settlement)
        assertEquals(1700000000L, settled.sessions.single().settledAt)
        assertEquals(true, settled.sessions.single().autoSettleDisabled)
    }

    @Test
    fun autoSettleSettingsReadAnOffInactivityRuleAsNull() {
        val off = WireJson.parseToJsonElement(
            """{"ok":true,"after_days":null,"on_merge":true,"keep_pull_requests":false}""",
        ).jsonObject.decodeReply<AutoSettleSettingsReply>()
        assertNull(off.afterDays)
        assertEquals(true, off.onMerge)
        val on = WireJson.parseToJsonElement(
            """{"ok":true,"after_days":3,"on_merge":false,"keep_pull_requests":true}""",
        ).jsonObject.decodeReply<AutoSettleSettingsReply>()
        assertEquals(3, on.afterDays)
        assertEquals(true, on.keepPullRequests)
    }
}
