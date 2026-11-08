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
    fun treeMarksFoldersOfProjectsAndTheHostThatKeepsThem() {
        val older = WireJson.parseToJsonElement(
            """{"ok":true,"folders":[{"id":"p","name":"Project","owner":"acme"}],"chats":[]}""",
        ).jsonObject.decodeReply<TreeReply>()
        assertEquals(false, older.folderGroups)
        assertEquals(false, older.folders.single().group)
        assertEquals("acme", older.folders.single().owner)

        val current = WireJson.parseToJsonElement(
            """{"ok":true,"folder_groups":true,"folders":[{"id":"g","name":"acme","group":true,"owner":"acme"},""" +
                """{"id":"p","name":"web","parent":"g"}],"chats":[]}""",
        ).jsonObject.decodeReply<TreeReply>()
        assertEquals(true, current.folderGroups)
        assertEquals(listOf(true, false), current.folders.map { it.group })
        assertNull(current.folders.last().owner)
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
    fun gitStatusSaysWhenToCommitAndWhenToLookForAPullRequest() {
        fun status(json: String) = WireJson.parseToJsonElement(json).jsonObject.decodeReply<GitStatusReply>()
        val dirty = status("""{"ok":true,"branch":"fix","base":"main","upstream":"origin/fix","ahead":0,"behind":0,"staged":1,"unstaged":2,"untracked":1,"conflicted":0,"clean":false}""")
        assertEquals(4, dirty.changed)
        assertEquals(true, dirty.canCommit)
        assertEquals(false, dirty.mayHavePullRequest)
        val conflicted = dirty.copy(conflicted = 1)
        assertEquals(false, conflicted.canCommit)
        val pushed = status("""{"ok":true,"branch":"fix","base":"main","upstream":"origin/fix","ahead":0,"behind":0,"staged":0,"unstaged":0,"untracked":0,"conflicted":0,"clean":true}""")
        assertEquals(true, pushed.mayHavePullRequest)
        assertEquals(false, pushed.copy(ahead = 1).mayHavePullRequest)
        assertEquals(false, pushed.copy(upstream = "").mayHavePullRequest)
        assertEquals(false, pushed.copy(branch = "main").mayHavePullRequest)
        assertEquals(false, pushed.copy(branch = "(detached)").mayHavePullRequest)
    }

    @Test
    fun newChatSourcesListBranchesAndPullRequests() {
        val sources = WireJson.parseToJsonElement(
            """{"ok":true,"branches":[{"name":"feature","reference":"refs/heads/feature","oid":"abc"}],"pull_requests":[{"number":7,"title":"Fix","oid":"def"}],"pull_request_error":null}""",
        ).jsonObject.decodeReply<NewChatSourcesReply>()
        assertEquals("refs/heads/feature", sources.branches.single().reference)
        assertEquals(7L, sources.pullRequests.single().number)
        assertNull(sources.pullRequestError)
        val settings = WireJson.parseToJsonElement(
            """{"ok":true,"can_create_worktree":true,"effective_workdir":"/repo","worktrees":[{"path":"/repo","branch":"main","detached":false,"main":true,"current":true}]}""",
        ).jsonObject.decodeReply<FolderSettingsReply>()
        assertEquals(true, settings.canCreateWorktree)
        assertEquals("/repo", settings.worktrees.single().path)
    }

    @Test
    fun usageSummaryAndLimitsParseTheHostsShapes() {
        val summary = WireJson.decodeFromJsonElement(
            UsageSummaryReply.serializer(),
            WireJson.parseToJsonElement(
                """{"read_at_ms":1,"time_zone":"UTC","resolution":"hour","window_label":"Past 24h","periods":[{"key":"2026-11-07T10:00:00Z","label":"10:00","tooltip":"1 hour ago"}],"buckets":[{"day":"2026-11-07","hour_start":"2026-11-07T10:00:00Z","provider":"codex","model":"gpt","totals":{"uncached_input":10,"cached_input":5,"cache_creation":0,"output":3,"reasoning":1},"cost_usd":0.25,"cache_savings_usd":0.0,"category_cost_usd":null,"cost_source":"modelPriced","records":2,"unpriced_records":0,"unpriced_tokens":0}],"sessions":{"codex":2},"sources":[],"pricing":{}}""",
            ),
        )
        assertEquals("2026-11-07T10:00:00Z", summary.buckets.single().period)
        assertEquals(18L, summary.buckets.single().totals.all)
        assertEquals(2, summary.sessions["codex"])
        val accounts = WireJson.decodeFromJsonElement(
            kotlinx.serialization.builtins.ListSerializer(UsageAccountReply.serializer()),
            WireJson.parseToJsonElement(
                """[{"provider":"codex","account_key":"abc","plan":"Pro","checked_at_ms":1,"windows":[{"id":"primary","kind":"session","label":"5 hours","used_percent":42.5,"resets_at_ms":2,"resets_at_label":"in 2 hours","window_duration_mins":300}],"reset_credits":{"available_count":1,"next_expires_at_ms":null}},{"provider":"claude","checked_at_ms":1,"windows":[],"unavailable":{"reason":"signed-out","message":"Sign in to read limits."}}]""",
            ),
        )
        assertEquals(42.5, accounts.first().windows.single().usedPercent)
        assertEquals(1, accounts.first().resetCredits!!.availableCount)
        assertEquals("Sign in to read limits.", accounts.last().unavailable!!.message)
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
