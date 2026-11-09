package com.restartfu.xd.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

class OpsTest {
    @Test
    fun newSessionsCanStartFromEachKindOfWorkspace() {
        fun worktree(choice: Ops.NewSessionWorktree) =
            Ops.newChat("f", backend = "codex", worktree = choice)["worktree"]!!.jsonObject
        assertEquals("new", worktree(Ops.NewSessionWorktree.New)["kind"]!!.jsonPrimitive.content)
        val existing = worktree(Ops.NewSessionWorktree.Existing("/repo-feature"))
        assertEquals("existing", existing["kind"]!!.jsonPrimitive.content)
        assertEquals("/repo-feature", existing["path"]!!.jsonPrimitive.content)
        val branch = worktree(Ops.NewSessionWorktree.Branch("refs/heads/feature", "abc"))
        assertEquals("branch", branch["kind"]!!.jsonPrimitive.content)
        assertEquals("refs/heads/feature", branch["ref"]!!.jsonPrimitive.content)
        assertEquals("abc", branch["oid"]!!.jsonPrimitive.content)
        val pull = worktree(Ops.NewSessionWorktree.PullRequest(42, "def"))
        assertEquals("pull-request", pull["kind"]!!.jsonPrimitive.content)
        assertEquals(42, pull["number"]!!.jsonPrimitive.int)
        assertFalse("worktree" in Ops.newChat("f"))
        // A chat joining a session shares its checkout.
        assertFailsWith<IllegalArgumentException> {
            Ops.newChat("f", sessionId = "s", worktree = Ops.NewSessionWorktree.New)
        }
    }

    @Test
    fun branchSessionsStartFromRemoteUnlessDisabled() {
        val branch = Ops.NewSessionWorktree.Branch("refs/heads/main", "abc")
        assertTrue(Ops.newChat("f", worktree = branch)["start_from_remote"]!!.jsonPrimitive.boolean)
        assertFalse(Ops.newChat("f", worktree = branch, startFromRemote = false)["start_from_remote"]!!.jsonPrimitive.boolean)
        for (choice in listOf(null, Ops.NewSessionWorktree.New, Ops.NewSessionWorktree.Existing("/repo"), Ops.NewSessionWorktree.PullRequest(1, "abc"))) {
            assertFalse("start_from_remote" in Ops.newChat("f", worktree = choice))
        }
    }

    @Test
    fun commitsTrimTheMessageAndRefuseOnesTheHostWould() {
        assertEquals("Fix login", Ops.gitCommit("c", "  Fix login \n")["message"]!!.jsonPrimitive.content)
        assertFailsWith<IllegalArgumentException> { Ops.gitCommit("c", "   ") }
        assertFailsWith<IllegalArgumentException> { Ops.gitCommit("c", "x".repeat(4097)) }
        assertEquals("git-pr-status", Ops.gitPullRequestStatus("c")["op"]!!.jsonPrimitive.content)
        assertEquals("folder-settings", Ops.folderSettings("f")["op"]!!.jsonPrimitive.content)
        assertEquals("new-chat-sources", Ops.newChatSources("f")["op"]!!.jsonPrimitive.content)
    }

    @Test
    fun hostSettingOperationsMatchTheHostsRules() {
        val start = Ops.agentAuthAction(Ops.AuthAction.START, "claude")
        assertEquals("agent-auth-start", start["op"]!!.jsonPrimitive.content)
        assertFalse("input" in start)
        assertEquals("1234", Ops.agentAuthAction(Ops.AuthAction.INPUT, "codex", "1234")["input"]!!.jsonPrimitive.content)
        assertFailsWith<IllegalArgumentException> { Ops.agentAuthAction(Ops.AuthAction.INPUT, "codex", "") }
        assertFailsWith<IllegalArgumentException> { Ops.agentAuthAction(Ops.AuthAction.START, "jcode") }
        assertFalse("account" in start)
        assertEquals("agent-auth-add", Ops.agentAuthAction(Ops.AuthAction.ADD, "codex")["op"]!!.jsonPrimitive.content)
        assertEquals("agent-auth-logout", Ops.agentAuthAction(Ops.AuthAction.LOGOUT, "codex")["op"]!!.jsonPrimitive.content)
        assertEquals("a1", Ops.agentAuthAction(Ops.AuthAction.START, "codex", account = "a1")["account"]!!.jsonPrimitive.content)
        assertEquals("a1", Ops.agentAuthAction(Ops.AuthAction.LOGOUT, "claude", account = "a1")["account"]!!.jsonPrimitive.content)
        // Input and cancelling belong to the provider's one running sign-in, not an account.
        assertFailsWith<IllegalArgumentException> { Ops.agentAuthAction(Ops.AuthAction.CANCEL, "codex", account = "a1") }
        assertFailsWith<IllegalArgumentException> { Ops.agentAuthAction(Ops.AuthAction.ADD, "codex", account = "a1") }
        assertFailsWith<IllegalArgumentException> { Ops.agentAuthAction(Ops.AuthAction.START, "codex", account = " ") }
        assertFailsWith<IllegalArgumentException> { Ops.agentCliUpdates("install") }
        assertEquals("codex", Ops.agentCliUpdates("install", "codex")["provider"]!!.jsonPrimitive.content)
        assertTrue(Ops.setChatTitleModel("codex", null)["model"] is JsonNull)
        assertFailsWith<IllegalArgumentException> { Ops.setChatTitleModel("jcode", "x") }
        assertTrue(Ops.setFolderContext("f", "   ")["context"] is JsonNull)
        assertEquals("notes", Ops.setFolderContext("f", " notes ")["context"]!!.jsonPrimitive.content)
        assertEquals(true, Ops.resumeInterrupted(true)["resume"]!!.jsonPrimitive.boolean)
    }

    @Test
    fun accountOrderNamesEachAccountOnce() {
        val order = Ops.agentAccountsOrder("codex", listOf("a1", "default"))
        assertEquals("agent-accounts-order", order["op"]!!.jsonPrimitive.content)
        assertEquals("codex", order["provider"]!!.jsonPrimitive.content)
        assertEquals(listOf("a1", "default"), order["accounts"]!!.jsonArray.map { it.jsonPrimitive.content })
        assertFailsWith<IllegalArgumentException> { Ops.agentAccountsOrder("codex", emptyList()) }
        assertFailsWith<IllegalArgumentException> { Ops.agentAccountsOrder("codex", listOf("a1", "a1")) }
        assertFailsWith<IllegalArgumentException> { Ops.agentAccountsOrder("codex", listOf("a1", "")) }
        assertFailsWith<IllegalArgumentException> { Ops.agentAccountsOrder("jcode", listOf("a1")) }
        val switch = Ops.agentAccountsAutoSwitch("claude", false)
        assertEquals("agent-accounts-auto-switch", switch["op"]!!.jsonPrimitive.content)
        assertEquals(false, switch["enabled"]!!.jsonPrimitive.boolean)
        assertFailsWith<IllegalArgumentException> { Ops.agentAccountsAutoSwitch("jcode", true) }
    }

    @Test
    fun secretsKeepStoredValuesBySendingNamesAlone() {
        val request = Ops.setAgentSecrets("f", listOf(Ops.SecretEntry("KEPT"), Ops.SecretEntry("NEW_ONE", "value")))
        val entries = request["entries"]!!.jsonArray.map { it.jsonObject }
        assertFalse("value" in entries[0])
        assertEquals("value", entries[1]["value"]!!.jsonPrimitive.content)
        assertFalse("folder" in Ops.setAgentSecrets(null, emptyList()))
        assertFailsWith<IllegalArgumentException> { Ops.setAgentSecrets(null, listOf(Ops.SecretEntry("1BAD", "v"))) }
        assertFailsWith<IllegalArgumentException> { Ops.setAgentSecrets(null, listOf(Ops.SecretEntry("A", "v"), Ops.SecretEntry("A", "w"))) }
        assertFailsWith<IllegalArgumentException> { Ops.setAgentSecrets(null, listOf(Ops.SecretEntry("A", ""))) }
    }

    @Test
    fun usageRequestsCarryTheirGenerationAndRefuseUnknownWindows() {
        val summary = Ops.usageSummary(7, "Europe/Paris", generation = 3)
        assertEquals("usage-summary", summary["op"]!!.jsonPrimitive.content)
        assertEquals(7, summary["days"]!!.jsonPrimitive.int)
        assertEquals(3, summary["generation"]!!.jsonPrimitive.int)
        assertFalse("refresh" in summary)
        assertEquals(true, Ops.usageSummary(1, "UTC", 4, refresh = true)["refresh"]!!.jsonPrimitive.boolean)
        assertFailsWith<IllegalArgumentException> { Ops.usageSummary(14, "UTC", 5) }
        assertEquals("usage-limits", Ops.usageLimits("UTC", 6)["op"]!!.jsonPrimitive.content)
        val reset = Ops.usageConsumeResetCredit("account", "attempt")
        assertEquals("codex", reset["provider"]!!.jsonPrimitive.content)
        assertEquals("attempt", reset["idempotency_key"]!!.jsonPrimitive.content)
        assertFailsWith<IllegalArgumentException> { Ops.usageConsumeResetCredit("", "attempt") }
        val prices = Ops.setUsagePrices(mapOf("local" to UsagePriceReply(2.0, 4.0)), mapOf("alias" to "local"))
        val local = prices["prices"]!!.jsonObject["local"]!!.jsonObject
        assertFalse("cache_read" in local)
        assertEquals("local", prices["aliases"]!!.jsonObject["alias"]!!.jsonPrimitive.content)
        assertFalse("aliases" in Ops.setUsagePrices(emptyMap()))
        assertFailsWith<IllegalArgumentException> { Ops.setUsagePrices(mapOf("m" to UsagePriceReply(-1.0, 1.0)), emptyMap()) }
    }

    @Test
    fun sessionOperationsNameTheSession() {
        assertEquals("settle-session", Ops.settleSession("s")["op"]!!.jsonPrimitive.content)
        assertEquals("s", Ops.unsettleSession("s")["session"]!!.jsonPrimitive.content)
        assertEquals("delete-session", Ops.deleteSession("s")["op"]!!.jsonPrimitive.content)
        val rename = Ops.renameSession("s", "Layout")
        assertEquals("rename-session", rename["op"]!!.jsonPrimitive.content)
        assertEquals("Layout", rename["title"]!!.jsonPrimitive.content)
        val auto = Ops.setSessionAutoSettle("s", enabled = false)
        assertEquals(false, auto["enabled"]!!.jsonPrimitive.boolean)
        assertFailsWith<IllegalArgumentException> { Ops.settleSession(" ") }
        assertFailsWith<IllegalArgumentException> { Ops.renameSession("s", " ") }
    }

    @Test
    fun autoSettleSettingsSendOnlyWhatChangedAndNullTurnsTheRuleOff() {
        val off = Ops.setAutoSettleSettings(afterDays = Ops.AfterDays.Off)
        assertTrue("after_days" in off)
        assertTrue(off["after_days"] is JsonNull)
        assertFalse("on_merge" in off)
        val some = Ops.setAutoSettleSettings(afterDays = Ops.AfterDays.Days(7), onMerge = true)
        assertEquals(7, some["after_days"]!!.jsonPrimitive.int)
        assertEquals(true, some["on_merge"]!!.jsonPrimitive.boolean)
        assertFalse("keep_pull_requests" in some)
        assertFailsWith<IllegalArgumentException> { Ops.setAutoSettleSettings(afterDays = Ops.AfterDays.Days(0)) }
        assertFailsWith<IllegalArgumentException> { Ops.setAutoSettleSettings(afterDays = Ops.AfterDays.Days(91)) }
    }

    @Test
    fun reorderQueueCarriesBothTextsForTheHostToCheck() {
        val request = Ops.reorderQueue("c", source = 2, sourceText = "third", anchor = 0, anchorText = "first", after = false)
        assertEquals("reorder-queue", request["op"]!!.jsonPrimitive.content)
        assertEquals(2, request["source"]!!.jsonPrimitive.int)
        assertEquals("third", request["source-text"]!!.jsonPrimitive.content)
        assertEquals("first", request["anchor-text"]!!.jsonPrimitive.content)
        assertEquals(false, request["after"]!!.jsonPrimitive.boolean)
        assertEquals("resume-turn", Ops.resumeTurn("c")["op"]!!.jsonPrimitive.content)
    }

    @Test
    fun projectOperationsRefuseHiddenOrNestedNames() {
        assertEquals("trash-folder", Ops.trashFolder("f")["op"]!!.jsonPrimitive.content)
        assertEquals("Atlas", Ops.renameFolder("f", "Atlas")["name"]!!.jsonPrimitive.content)
        assertFailsWith<IllegalArgumentException> { Ops.renameFolder("f", ".hidden") }
        assertFailsWith<IllegalArgumentException> { Ops.renameFolder("f", "a/b") }
    }

    @Test
    fun shortcutOperationsDistinguishGlobalAndWorkspaceScope() {
        val global = Ops.setShortcuts(prompts = listOf("Run tests"))
        assertEquals("set-shortcuts", global.getValue("op").jsonPrimitive.content)
        assertFalse(global.containsKey("folder"))
        assertEquals(
            "Run tests",
            global.getValue("shortcuts").jsonArray.single().jsonPrimitive.content,
        )

        val workspace = Ops.shortcuts("folder-1")
        assertEquals("shortcuts", workspace.getValue("op").jsonPrimitive.content)
        assertEquals("folder-1", workspace.getValue("folder").jsonPrimitive.content)
    }

    @Test
    fun booleanOptionsAreStringsOnWire() {
        val request = Ops.setBoolOption("chat", ChatOption.PLAN, true)

        assertEquals("true", request.getValue("value").jsonPrimitive.content)
        assertTrue(request.toString().contains("\"value\":\"true\""))
        assertEquals("fast", ChatOption.FAST.wire)
    }

    @Test
    fun subagentActionsNameTheAgentAndRejectUnknownVerbs() {
        val stop = Ops.subagent("chat", "toolu_1", "stop")
        assertEquals("subagent", stop.getValue("op").jsonPrimitive.content)
        assertEquals("toolu_1", stop.getValue("key").jsonPrimitive.content)
        assertEquals("stop", stop.getValue("action").jsonPrimitive.content)
        assertFailsWith<IllegalArgumentException> { Ops.subagent("chat", "toolu_1", "pause") }
    }

    @Test
    fun dropQueueDistinguishesOneFromAll() {
        assertFalse("index" in Ops.dropQueue("chat", null))
        assertEquals(2, Ops.dropQueue("chat", 2).getValue("index").jsonPrimitive.content.toInt())
        assertFalse("text" in Ops.dropQueue("chat", 2))
        assertEquals("second", Ops.dropQueue("chat", 2, "second").getValue("text").jsonPrimitive.content)
        assertFalse("text" in Ops.dropQueue("chat", null, "ignored"))
        assertEquals(7, Ops.dropQueue("chat", 0, "repeat", 7)["queue_revision"]!!.jsonPrimitive.int)
        assertFalse("queue_revision" in Ops.dropQueue("chat", 0, "repeat"))
        assertEquals(3, Ops.dropQueue("chat", null, revision = 3)["queue_revision"]!!.jsonPrimitive.int)
    }

    @Test
    fun pngAttachmentIsEncoded() {
        val png = PngAttachment(PNG_HEADER + byteArrayOf(1, 2, 3))
        val request = Ops.send("chat", "", listOf(png))
        val attachments = request.getValue("attachments").jsonArray

        assertEquals(1, attachments.size)
        assertTrue(request.toString().contains("\"mime\":\"image/png\""))
    }

    @Test
    fun draftTextOmitsUnchangedAttachmentsAndCanReplaceThem() {
        val png = PngAttachment(PNG_HEADER + byteArrayOf(4, 5, 6))
        val text = Ops.setDraft("chat", "typing")
        val images = Ops.setDraft("chat", "typing", listOf(png))

        assertFalse("attachments" in text)
        assertEquals(1, images.getValue("attachments").jsonArray.size)
        assertEquals("set-draft", images.getValue("op").jsonPrimitive.content)
    }

    @Test
    fun invalidImageFailsBeforeEncoding() {
        assertFailsWith<IllegalArgumentException> {
            Ops.send("chat", "", listOf(PngAttachment(byteArrayOf(1, 2, 3))))
        }
    }

    @Test
    fun newChatRequiresAFolder() {
        assertFailsWith<IllegalArgumentException> {
            Ops.newChat("")
        }
        assertEquals(
            "folder",
            Ops.newChat("folder").getValue("folder").jsonPrimitive.content,
        )
    }

    @Test
    fun newChatCarriesFirstTurnOptions() {
        val request = Ops.newChat("folder", effort = "high", access = "edit", plan = true)
        assertEquals("high", request.getValue("effort").jsonPrimitive.content)
        assertEquals("edit", request.getValue("access").jsonPrimitive.content)
        assertEquals("true", request.getValue("plan").jsonPrimitive.content)
        assertFalse("effort" in Ops.newChat("folder"))
    }

    @Test
    fun newChatCanJoinAnExistingWorkspaceSession() {
        val request = Ops.newChat("project", "Review", "claude", sessionId = "session")
        assertEquals("session", request.getValue("session").jsonPrimitive.content)
        assertEquals("claude", request.getValue("backend").jsonPrimitive.content)
        assertFalse("workdir" in request)
        assertFalse("session" in Ops.newChat("project"))
        assertFailsWith<IllegalArgumentException> { Ops.newChat("project", sessionId = " ") }
    }

    @Test
    fun newFolderDistinguishesAWorkspaceFromANestedFolder() {
        val workspace = Ops.newFolder("Mobile")
        val nested = Ops.newFolder("App", "workspace")
        val connected = Ops.newFolder(
            "Connected",
            repository = "/home/user/project",
        )

        assertEquals("new-folder", workspace.getValue("op").jsonPrimitive.content)
        assertEquals("Mobile", workspace.getValue("name").jsonPrimitive.content)
        assertFalse("parent" in workspace)
        assertEquals("workspace", nested.getValue("parent").jsonPrimitive.content)
        assertEquals(
            "/home/user/project",
            connected.getValue("repo").jsonPrimitive.content,
        )

        val cloned = Ops.newFolder(
            "Cloned",
            repositoryUrl = "https://github.com/owner/repo.git",
        )
        assertEquals(
            "https://github.com/owner/repo.git",
            cloned.getValue("repo_url").jsonPrimitive.content,
        )
        assertFalse("repo_url" in connected)
        assertFailsWith<IllegalArgumentException> { Ops.newFolder(" ") }
        assertFailsWith<IllegalArgumentException> { Ops.newFolder(".hidden") }
        assertFailsWith<IllegalArgumentException> { Ops.newFolder("Mobile/App") }
        assertFailsWith<IllegalArgumentException> { Ops.newFolder("Mobile\\App") }
        assertFalse("group" in workspace)
        assertFalse("owner" in workspace)
    }

    @Test
    fun newFolderMarksAFolderOfProjectsAndItsOrganization() {
        val folder = Ops.newFolder("Clients", "workspace", group = true)
        assertEquals(true, folder.getValue("group").jsonPrimitive.content.toBoolean())
        assertFalse("owner" in folder)
        assertEquals("workspace", folder.getValue("parent").jsonPrimitive.content)

        // An organization is always a folder, even if the caller forgets to say so.
        val organization = Ops.newFolder("acme", owner = "acme")
        assertEquals(true, organization.getValue("group").jsonPrimitive.content.toBoolean())
        assertEquals("acme", organization.getValue("owner").jsonPrimitive.content)

        assertFailsWith<IllegalArgumentException> {
            Ops.newFolder("Clients", repository = "/home/user/project", group = true)
        }
        assertFailsWith<IllegalArgumentException> {
            Ops.newFolder("acme", repositoryUrl = "https://github.com/acme/web.git", owner = "acme")
        }
    }

    @Test
    fun ownerIconNamesTheAccount() {
        val request = Ops.ownerIcon("RestartFU")
        assertEquals("owner-icon", request.getValue("op").jsonPrimitive.content)
        assertEquals("RestartFU", request.getValue("owner").jsonPrimitive.content)
        assertFailsWith<IllegalArgumentException> { Ops.ownerIcon(" ") }
        assertFailsWith<IllegalArgumentException> { Ops.newFolder("App", " ") }
    }

    @Test
    fun directoryListingUsesTheHostFilesystem() {
        val home = Ops.listDirectories()
        val nested = Ops.listDirectories("/home/user")

        assertEquals("list-dir", home.getValue("op").jsonPrimitive.content)
        assertFalse("path" in home)
        assertEquals("/home/user", nested.getValue("path").jsonPrimitive.content)
    }

    @Test
    fun moveOperationsOmitRootParentsAndRequireIds() {
        val folderToRoot = Ops.moveFolder("folder")
        val folderNested = Ops.moveFolder("folder", "parent")
        val chat = Ops.moveChat("chat", "folder")

        assertEquals("move-folder", folderToRoot.getValue("op").jsonPrimitive.content)
        assertFalse("parent" in folderToRoot)
        assertEquals("parent", folderNested.getValue("parent").jsonPrimitive.content)
        assertEquals("move-chat", chat.getValue("op").jsonPrimitive.content)
        assertEquals("chat", chat.getValue("chat").jsonPrimitive.content)
        assertEquals("folder", chat.getValue("folder").jsonPrimitive.content)
        assertFailsWith<IllegalArgumentException> { Ops.moveFolder("") }
        assertFailsWith<IllegalArgumentException> { Ops.moveFolder("folder", " ") }
        assertFailsWith<IllegalArgumentException> { Ops.moveChat("", "folder") }
        assertFailsWith<IllegalArgumentException> { Ops.moveChat("chat", "") }
    }

    @Test
    fun selectingAModelCannotChangeTheChatsAssistant() {
        val request = Ops.selectModel("chat-1", "gpt-5.5")

        assertEquals("set-option", request.getValue("op").jsonPrimitive.content)
        assertEquals("model", request.getValue("option").jsonPrimitive.content)
        assertFalse("backend" in request)
        assertEquals("gpt-5.5", request.getValue("value").jsonPrimitive.content)
    }

    @Test
    fun aBlankModelIsRejected() {
        assertFailsWith<IllegalArgumentException> {
            Ops.selectModel("chat-1", " ")
        }
    }

    @Test
    fun steerCarriesTheTextTheHostMatchesOn() {
        // The host refuses a steer whose text is not what is queued, so the
        // guard has to travel with the request.
        val request = Ops.steerQueue("chat-1", 2, "do this instead")

        assertEquals("steer-queue", request.getValue("op").jsonPrimitive.content)
        assertEquals(2, request.getValue("index").jsonPrimitive.int)
        assertEquals(
            "do this instead",
            request.getValue("text").jsonPrimitive.content,
        )
    }

    @Test
    fun editCarriesBothTheOldAndNewText() {
        val request = Ops.editQueue("chat-1", 0, "before", "after")

        assertEquals("edit-queue", request.getValue("op").jsonPrimitive.content)
        assertEquals("before", request.getValue("old-text").jsonPrimitive.content)
        assertEquals("after", request.getValue("text").jsonPrimitive.content)
    }

    @Test
    fun queueEditsRejectImpossibleArguments() {
        assertFailsWith<IllegalArgumentException> {
            Ops.steerQueue("chat-1", -1, "x")
        }
        assertFailsWith<IllegalArgumentException> {
            Ops.editQueue("chat-1", 0, "before", "")
        }
    }

    @Test
    fun workflowStatusCarriesTheCapturedMarker() {
        val marker = "workflow_run\n123\n" +
            "https://github.com/RestartFU/xd/actions/runs/123"
        val request = Ops.workflowStatus(marker)

        assertEquals("workflow-status", request.getValue("op").jsonPrimitive.content)
        assertEquals(marker, request.getValue("text").jsonPrimitive.content)
        assertFailsWith<IllegalArgumentException> { Ops.workflowStatus(" ") }
    }

    private companion object {
        val PNG_HEADER = byteArrayOf(
            0x89.toByte(),
            0x50,
            0x4e,
            0x47,
            0x0d,
            0x0a,
            0x1a,
            0x0a,
        )
    }
}
