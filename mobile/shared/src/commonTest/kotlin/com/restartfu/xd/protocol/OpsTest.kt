package com.restartfu.xd.protocol

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue
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
