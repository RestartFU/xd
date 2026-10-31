package com.restartfu.xd.model

import com.restartfu.xd.protocol.BackendReply
import com.restartfu.xd.protocol.ModelReply
import kotlin.test.Test
import kotlin.test.assertEquals

class DisplayLabelsTest {
    @Test
    fun hostModelNamesAreScopedToTheSelectedBackend() {
        val catalog = listOf(
            BackendReply("codex", "Codex", models = listOf(ModelReply("shared-id", "Coding model"))),
            BackendReply("claude", "Claude Code", models = listOf(ModelReply("shared-id", "Review model"))),
        )
        assertEquals("Review model", DisplayLabels.model("shared-id", "claude", catalog))
        assertEquals("Coding model", DisplayLabels.model("shared-id", "codex", catalog))
        assertEquals("shared-id", DisplayLabels.model("shared-id", "custom", catalog))
    }

    @Test
    fun catalogNamesOverrideKnownFallbacksAndUnknownIdsStayIntact() {
        val catalog = listOf(BackendReply("codex", "Codex", models = listOf(
            ModelReply("gpt-5.3-codex", "Team coding model"),
        )))
        assertEquals("Team coding model", DisplayLabels.model("gpt-5.3-codex", "codex", catalog))
        assertEquals("GPT-5.3 Codex", DisplayLabels.model("gpt-5.3-codex"))
        assertEquals("Claude Haiku 4.5", DisplayLabels.model("claude-haiku-4-5"))
        assertEquals("Automatic", DisplayLabels.model("default"))
        assertEquals("provider/team-model_v2", DisplayLabels.model("provider/team-model_v2"))
        assertEquals("Default model", DisplayLabels.model(null))
    }

    @Test
    fun accessAliasesShareSelectionAndDoNotRewriteUnknownValues() {
        assertEquals("edit", DisplayLabels.canonicalAccess("workspace-write"))
        assertEquals("Edit files", DisplayLabels.access("workspace-write"))
        assertEquals(DisplayLabels.access("full"), DisplayLabels.access("danger-full-access"))
        assertEquals("read-only", DisplayLabels.canonicalAccess("readonly"))
        assertEquals("Read only", DisplayLabels.access("readonly"))
        assertEquals("custom-policy", DisplayLabels.canonicalAccess("custom-policy"))
        assertEquals("custom-policy", DisplayLabels.access("custom-policy"))
    }

    @Test
    fun effortAndBackendLabelsUseConsistentHumanNames() {
        assertEquals("Extra high", DisplayLabels.effort("xhigh"))
        assertEquals("Maximum", DisplayLabels.effort("max"))
        assertEquals("Ultra code", DisplayLabels.effort("ultracode"))
        assertEquals("Default", DisplayLabels.effort(" "))
        assertEquals("custom-effort", DisplayLabels.effort("custom-effort"))
        assertEquals("Claude Code", DisplayLabels.backend("claude"))
        assertEquals("custom-agent", DisplayLabels.backend("custom-agent"))
    }
}
