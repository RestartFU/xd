package com.restartfu.xd.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive

class OpsTest {
    @Test
    fun pairingSendsTheConnectingDeviceName() {
        val request = Ops.pair("ABCD-EFGH", "Pixel 9")

        assertEquals("pair", request.getValue("op").jsonPrimitive.content)
        assertEquals("ABCD-EFGH", request.getValue("code").jsonPrimitive.content)
        assertEquals("Pixel 9", request.getValue("name").jsonPrimitive.content)
    }

    @Test
    fun booleanOptionsAreStringsOnWire() {
        val request = Ops.setBoolOption("chat", ChatOption.PLAN, true)

        assertEquals("true", request.getValue("value").jsonPrimitive.content)
        assertTrue(request.toString().contains("\"value\":\"true\""))
        assertEquals("fast", ChatOption.FAST.wire)
        assertEquals("claude-mode", ChatOption.CLAUDE_MODE.wire)
    }

    @Test
    fun dropQueueDistinguishesOneFromAll() {
        assertFalse("index" in Ops.dropQueue("chat", null))
        assertEquals(2, Ops.dropQueue("chat", 2).getValue("index").jsonPrimitive.content.toInt())
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
        assertFailsWith<IllegalArgumentException> { Ops.newFolder("App", " ") }
    }

    @Test
    fun directoryListingUsesTheDaemonFilesystem() {
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
    fun selectingAModelSendsItsAssistantToo() {
        // Without a backend the daemon stores the string unvalidated and skips
        // the effort reconciliation and the visible switch event.
        val request = Ops.selectModel("chat-1", "codex", "gpt-5.5")

        assertEquals("set-option", request.getValue("op").jsonPrimitive.content)
        assertEquals("model", request.getValue("option").jsonPrimitive.content)
        assertEquals("codex", request.getValue("backend").jsonPrimitive.content)
        assertEquals("gpt-5.5", request.getValue("value").jsonPrimitive.content)
    }

    @Test
    fun aModelWithoutAnAssistantIsRejected() {
        assertFailsWith<IllegalArgumentException> {
            Ops.selectModel("chat-1", "", "gpt-5.5")
        }
        assertFailsWith<IllegalArgumentException> {
            Ops.selectModel("chat-1", "codex", " ")
        }
    }

    @Test
    fun steerCarriesTheTextTheDaemonMatchesOn() {
        // The daemon refuses a steer whose text is not what is queued, so the
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
