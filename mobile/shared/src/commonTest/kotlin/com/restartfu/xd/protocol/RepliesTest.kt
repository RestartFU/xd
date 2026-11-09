package com.restartfu.xd.protocol

import kotlinx.serialization.json.jsonObject
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

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
    fun treeListsEachChatsLinkedPullRequestsOnHostsThatKeepThem() {
        val older = WireJson.parseToJsonElement(
            """{"ok":true,"folders":[],"chats":[{"id":"one","folder":"p","title":"One","backend":"codex","working":false}]}""",
        ).jsonObject.decodeReply<TreeReply>()
        assertFalse(older.linkedPullRequests)
        assertEquals(emptyList(), older.chats.single().pullRequests)

        val current = WireJson.parseToJsonElement(
            """{"ok":true,"linked_pull_requests":true,"folders":[],"chats":[""" +
                """{"id":"one","folder":"p","title":"One","backend":"codex","working":false,"pull_requests":[""" +
                """{"url":"https://github.com/o/r/pull/7","state":"merged"},{"url":"https://github.com/o/r/pull/9"}]},""" +
                """{"id":"two","folder":"p","title":"Two","backend":"codex","working":false}]}""",
        ).jsonObject.decodeReply<TreeReply>()
        assertTrue(current.linkedPullRequests)
        assertEquals(
            listOf(
                LinkedPullRequestReply("https://github.com/o/r/pull/7", "merged"),
                LinkedPullRequestReply("https://github.com/o/r/pull/9", null),
            ),
            current.chats.first().pullRequests,
        )
        assertEquals(emptyList(), current.chats.last().pullRequests)
    }

    @Test
    fun linkingAnswersWhatItKeptAndWhatItRefused() {
        val linked = WireJson.parseToJsonElement(
            """{"ok":true,"linked":["https://github.com/o/r/pull/7"],"invalid":["nope"]}""",
        ).jsonObject.decodeReply<LinkPullRequestsReply>()
        assertEquals(listOf("https://github.com/o/r/pull/7"), linked.linked)
        assertEquals(listOf("nope"), linked.invalid)
        val bare = WireJson.parseToJsonElement("""{"ok":true}""").jsonObject.decodeReply<LinkPullRequestsReply>()
        assertEquals(emptyList(), bare.linked)
        assertEquals(emptyList(), bare.invalid)
        val refused = runCatching {
            WireJson.parseToJsonElement("""{"ok":false,"error":"unknown chat"}""").jsonObject.decodeReply<LinkPullRequestsReply>()
        }.exceptionOrNull()
        assertTrue(refused is RemoteRefusedException)
        assertEquals("unknown chat", refused.message)

        assertTrue(WireJson.parseToJsonElement("""{"ok":true,"changed":true}""").jsonObject.decodeReply<UnlinkPullRequestReply>().changed)
        assertFalse(WireJson.parseToJsonElement("""{"ok":true,"changed":false}""").jsonObject.decodeReply<UnlinkPullRequestReply>().changed)
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

    @Test
    fun aHostFromBeforeAccountsShowsItsOneSignIn() {
        val reply = WireJson.parseToJsonElement(
            """{"ok":true,"providers":[{"provider":"codex","display_name":"Codex","state":"signing-in",""" +
                """"login_url":"https://auth.example.com/device","device_code":"ABCD-1234"}]}""",
        ).jsonObject.decodeReply<AgentAuthReply>()
        assertEquals(false, reply.multipleAccounts)
        val codex = reply.providers.single()
        assertFalse(codex.holdsSeveral)
        assertFalse(codex.canAdd)
        val only = codex.accountList().single()
        assertEquals(AgentAccountReply.DEFAULT, only.id)
        assertTrue(only.default && only.inUse)
        assertEquals("Codex sign-in", only.label)
        assertEquals("ABCD-1234", codex.signingIn()?.deviceCode)
        assertTrue(codex.busy)
        assertNull(codex.orderMoving(0, up = false))
    }

    @Test
    fun accountsCarryTheirOrderLimitsAndSignIn() {
        val reply = WireJson.parseToJsonElement(
            """{"ok":true,"multiple_accounts":true,"providers":[{"provider":"codex","display_name":"Codex",""" +
                """"state":"signed-in","needs_input":false,"auto_switch":true,"adding":true,"notice":"Already in the list.",""" +
                """"accounts":[""" +
                """{"id":"default","default":true,"pending":false,"label":"dana@example.com","email":"dana@example.com",""" +
                """"plan":"Pro","state":"signed-in","needs_input":false,"in_use":false,"busy":false,"problem":null,""" +
                """"limited_until_ms":7500000,"limited_reason":"5-hour limit","checked_at_ms":5,""" +
                """"windows":[{"kind":"session","label":"5h","used_percent":100.0,"resets_at_ms":7500000}]},""" +
                """{"id":"a1","default":false,"pending":false,"label":"dana.work@example.com","plan":"Plus",""" +
                """"state":"signed-in","in_use":true,"problem":"mismatch","windows":[]},""" +
                """{"id":"a2","default":false,"pending":true,"label":"Codex account","state":"signing-in",""" +
                """"device_code":"WXYZ-9876","login_url":"https://auth.example.com/device","needs_input":false,"busy":true,"windows":[]}""" +
                """]}]}""",
        ).jsonObject.decodeReply<AgentAuthReply>()
        assertTrue(reply.multipleAccounts)
        val codex = reply.providers.single()
        assertTrue(codex.holdsSeveral && codex.autoSwitch && codex.adding)
        assertEquals("Already in the list.", codex.notice)
        val (first, second) = codex.accountList()
        assertEquals(7_500_000L, first.limitedUntilMs)
        assertEquals("5-hour limit", first.limitedReason)
        assertEquals(100.0, first.windows.single().usedPercent)
        assertEquals("mismatch", second.problem)
        assertTrue(second.inUse)
        assertEquals("a2", codex.signingIn()?.id)
        assertTrue(codex.busy)
        assertFalse(codex.canAdd)
        // One still signing in for the first time has no place in the order.
        assertEquals(listOf("default", "a1"), codex.rankedAccounts().map { it.id })
        assertEquals(listOf("a1", "default"), codex.orderMoving(1, up = true))
        assertEquals(listOf("a1", "default"), codex.orderMoving(0, up = false))
        assertNull(codex.orderMoving(0, up = true))
        assertNull(codex.orderMoving(1, up = false))
    }

    @Test
    fun usageLimitsSayWhichOfSeveralAccountsTheyAre() {
        val accounts = WireJson.decodeFromJsonElement(
            kotlinx.serialization.builtins.ListSerializer(UsageAccountReply.serializer()),
            WireJson.parseToJsonElement(
                """[{"provider":"codex","account_key":"a","account_rank":1,"windows":[]},""" +
                    """{"provider":"codex","account_key":"b","account_rank":2,"windows":[]},""" +
                    """{"provider":"claude","account_key":"c","windows":[]}]""",
            ),
        )
        assertEquals(listOf(1, 2, null), accounts.map { it.accountRank })
    }
}
