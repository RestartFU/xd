package com.restartfu.xd.model

import com.restartfu.xd.protocol.WireJson
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.jsonObject

class ChatQuotaTest {
    private fun parse(json: String) = ChatQuota.parse(WireJson.parseToJsonElement(json))

    @Test
    fun aQuotaReadsItsShareUseAndOwner() {
        assertEquals(ChatQuota(10, 4.5, own = true), parse("""{"percent":10,"used":4.5,"own":true}"""))
        assertEquals(ChatQuota(20, 0.0, own = false), parse("""{"percent":20}"""))
        assertEquals("5", ChatQuota(10, 4.5).usedLabel)
        assertEquals("12", ChatQuota(10, 12.2).usedLabel)
    }

    @Test
    fun anythingButAWholePercentageFromOneToAHundredIsNoQuota() {
        assertNull(ChatQuota.parse(null))
        assertNull(ChatQuota.parse(JsonNull))
        assertNull(parse("""[10]"""))
        assertNull(parse("""{"used":3}"""))
        assertNull(parse("""{"percent":null}"""))
        assertNull(parse("""{"percent":"10"}"""))
        assertNull(parse("""{"percent":0}"""))
        assertNull(parse("""{"percent":101}"""))
        assertNull(parse("""{"percent":2.5}"""))
        // An odd member besides a good percent keeps the quota.
        assertEquals(ChatQuota(10, 0.0, own = false), parse("""{"percent":10,"used":"x","own":"yes"}"""))
        assertEquals(ChatQuota(10, 0.0), parse("""{"percent":10,"used":-1}"""))
    }

    @Test
    fun theNextQuotaOfferedIsTwiceTheLastUpToTheWholeLimit() {
        assertEquals(20, ChatQuota.nextPercent(10))
        assertEquals(2, ChatQuota.nextPercent(1))
        assertEquals(100, ChatQuota.nextPercent(50))
        assertEquals(100, ChatQuota.nextPercent(80))
        assertEquals(100, ChatQuota.nextPercent(100))
        assertEquals(ChatQuota.DEFAULT_PERCENT, ChatQuota.nextPercent(null))
    }

    @Test
    fun aTypedQuotaIsAWholeNumberFromOneToAHundred() {
        assertEquals(25, ChatQuota.typedPercent("25"))
        assertEquals(25, ChatQuota.typedPercent(" 25 % "))
        assertEquals(100, ChatQuota.typedPercent("100"))
        assertNull(ChatQuota.typedPercent(""))
        assertNull(ChatQuota.typedPercent("0"))
        assertNull(ChatQuota.typedPercent("101"))
        assertNull(ChatQuota.typedPercent("2.5"))
    }

    @Test
    fun theDefaultIsReadOnlyFromAReplyThatNamesIt() {
        fun settings(json: String) = ChatQuotaSettings.parse(WireJson.parseToJsonElement(json).jsonObject)
        assertEquals(ChatQuotaSettings(15), settings("""{"ok":true,"percent":15}"""))
        assertEquals(ChatQuotaSettings(null), settings("""{"ok":true,"percent":null}"""))
        assertEquals(ChatQuotaSettings(null), settings("""{"event":"chat-quota-settings","percent":"bad"}"""))
        assertNull(settings("""{"ok":true}"""))
    }
}
