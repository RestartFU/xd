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
    fun booleanOptionsAreStringsOnWire() {
        val request = Ops.setBoolOption("chat", ChatOption.PLAN, true)

        assertEquals("true", request.getValue("value").jsonPrimitive.content)
        assertTrue(request.toString().contains("\"value\":\"true\""))
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
