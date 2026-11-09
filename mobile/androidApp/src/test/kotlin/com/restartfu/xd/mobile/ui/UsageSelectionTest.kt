package com.restartfu.xd.mobile.ui

import com.restartfu.xd.protocol.UsageBucketReply
import com.restartfu.xd.protocol.UsageSourceReply
import com.restartfu.xd.protocol.UsageSummaryReply
import org.junit.Assert.assertEquals
import org.junit.Test

class UsageSelectionTest {
    private val summary = UsageSummaryReply(
        buckets = listOf(
            UsageBucketReply("day", provider = "codex", model = "shared", costUsd = 3.0),
            UsageBucketReply("day", provider = "claude", model = "shared", costUsd = 5.0),
            UsageBucketReply("day", provider = "claude", model = "other", costUsd = 7.0),
        ),
        sessions = mapOf("codex" to 2, "claude" to 4),
        sources = listOf(UsageSourceReply("codex"), UsageSourceReply("claude", status = "partial")),
    )

    @Test fun providerFiltersTotalsSessionsAndWarningsTogether() {
        val selected = summary.filtered("Codex", null, null)
        assertEquals(3.0, selected.buckets.sumOf { it.costUsd }, 0.0)
        assertEquals(mapOf("codex" to 2), selected.sessions)
        assertEquals(listOf("codex"), selected.sources.map { it.provider })
        assertEquals(summary, summary.filtered("All providers", null, null))
    }

    @Test fun modelIdentityIncludesProvider() {
        val selected = summary.filtered("All providers", "claude", "shared")
        assertEquals(5.0, selected.buckets.sumOf { it.costUsd }, 0.0)
        assertEquals(listOf("claude"), selected.sources.map { it.provider })
        assertEquals(emptyList<UsageBucketReply>(), summary.filtered("Codex", "claude", "shared").buckets)
    }
}
