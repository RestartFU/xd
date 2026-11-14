package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject

class ContextCompactionTest {
    @Test
    fun tokensReadAsT3CodeWritesThem() {
        assertEquals("0", formatTokens(0))
        assertEquals("950", formatTokens(950))
        assertEquals("999", formatTokens(999))
        assertEquals("1K", formatTokens(1_000))
        assertEquals("1.25K", formatTokens(1_250))
        assertEquals("1.50K", formatTokens(1_500))
        assertEquals("12.5K", formatTokens(12_500))
        assertEquals("32K", formatTokens(32_000))
        assertEquals("250K", formatTokens(250_000))
        assertEquals("251K", formatTokens(251_000))
        assertEquals("1M", formatTokens(999_999))
        assertEquals("1M", formatTokens(1_000_000))
        assertEquals("2.50B", formatTokens(2_500_000_000))
    }

    @Test
    fun theOfferIsReadFromTheChatReply() {
        assertEquals(
            CompactBeforeSend(251_000, 1_763_100_000_000),
            CompactBeforeSend.parse(json("""{"tokens":251000,"after_ms":1763100000000}""")),
        )
        assertNull(CompactBeforeSend.parse(null))
        assertNull(CompactBeforeSend.parse(json("""{"tokens":"251000","after_ms":1}""")))
        assertNull(CompactBeforeSend.parse(json("""{"tokens":251000}""")))
        assertNull(CompactBeforeSend.parse(json("""{"tokens":0,"after_ms":1}""")))
    }

    @Test
    fun theComposerOffersItOnlyWhenEverythingHolds() {
        val offer = CompactBeforeSend(251_000, afterMillis = 1_000)
        fun offered(
            now: Long = 1_000,
            mode: IdleCompactionMode? = IdleCompactionMode.ON_RETURN,
            working: Boolean = false,
            waiting: Boolean = false,
            text: String = "go on",
        ) = offer.offered(now, mode, working, waiting, text)

        assertTrue(offered())
        assertTrue(offered(text = ""))
        assertFalse(offered(now = 999))
        assertFalse(offered(mode = null))
        assertFalse(offered(mode = IdleCompactionMode.BEFORE_CACHE_EXPIRES))
        assertFalse(offered(mode = IdleCompactionMode.OFF))
        assertFalse(offered(working = true))
        assertFalse(offered(waiting = true))
        assertFalse(offered(text = "/compact"))
        assertFalse(offered(text = "  /COMPACT keep the plan"))
    }

    @Test
    fun modesReadTheHostsNames() {
        assertEquals(IdleCompactionSettings(IdleCompactionMode.ON_RETURN), IdleCompactionSettings.parse(json("""{"ok":true,"mode":"onReturn"}""")))
        assertEquals(
            IdleCompactionSettings(IdleCompactionMode.BEFORE_CACHE_EXPIRES),
            IdleCompactionSettings.parse(json("""{"event":"idle-compaction-settings","mode":"beforeCacheExpires"}""")),
        )
        assertEquals(IdleCompactionSettings(IdleCompactionMode.OFF), IdleCompactionSettings.parse(json("""{"mode":"off"}""")))
        assertNull(IdleCompactionSettings.parse(json("""{"ok":true}""")))
        assertNull(IdleCompactionSettings.parse(json("""{"mode":"sometimes"}""")))
    }

    private fun json(text: String) = Json.parseToJsonElement(text).jsonObject
}
