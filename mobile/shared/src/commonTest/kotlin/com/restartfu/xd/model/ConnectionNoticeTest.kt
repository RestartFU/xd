package com.restartfu.xd.model

import com.restartfu.xd.net.FatalReason
import com.restartfu.xd.net.Link
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ConnectionNoticeTest {
    @Test
    fun aWorkingLinkSaysNothing() {
        assertNull(connectionNotice(Link.Up("host"), lastFailure = null))
        // Nor does one that just came back after failing.
        assertNull(connectionNotice(Link.Up("host"), lastFailure = "offline"))
    }

    @Test
    fun aFatalLinkHasItsOwnScreen() {
        assertNull(connectionNotice(Link.Fatal(FatalReason.PROTOCOL, "bad reply"), lastFailure = null))
    }

    @Test
    fun aFirstConnectionIsToldQuietly() {
        for (link in listOf(Link.Idle, Link.Connecting)) {
            val notice = connectionNotice(link, lastFailure = null)!!
            assertEquals("Connecting…", notice.text)
            assertFalse(notice.failure)
            assertEquals("Connecting to the host", notice.spoken)
        }
    }

    @Test
    fun aLostLinkSaysWhyAndWhenItTriesAgain() {
        val notice = connectionNotice(Link.Down("Connection refused.", 15_000), lastFailure = null, secondsLeft = 15)!!
        assertTrue(notice.failure)
        assertEquals("Reconnecting in 15 s · Connection refused", notice.text)
    }

    @Test
    fun theCountdownIsNotSpoken() {
        val first = connectionNotice(Link.Down("offline", 15_000), null, secondsLeft = 15)!!
        val later = connectionNotice(Link.Down("offline", 15_000), null, secondsLeft = 3)!!
        assertEquals("Not connected to the host: offline. Reconnecting.", first.spoken)
        assertEquals(first.spoken, later.spoken)
    }

    @Test
    fun longWaitsCountInMinutes() {
        assertEquals("Reconnecting in 2 min", connectionNotice(Link.Down("", 120_000), null, secondsLeft = 120)!!.title)
        assertEquals("Reconnecting in 2 min", connectionNotice(Link.Down("", 120_000), null, secondsLeft = 61)!!.title)
        assertEquals("Reconnecting in 59 s", connectionNotice(Link.Down("", 120_000), null, secondsLeft = 59)!!.title)
    }

    @Test
    fun theAttemptAfterAWaitKeepsTheReason() {
        val notice = connectionNotice(Link.Connecting, lastFailure = "offline")!!
        assertTrue(notice.failure)
        assertEquals("Reconnecting… · offline", notice.text)
        // The wait ran out before the link moved on.
        assertEquals("Reconnecting…", connectionNotice(Link.Down("offline", 2_000), null, secondsLeft = 0)!!.title)
    }

    @Test
    fun aBlankReasonIsLeftOut() {
        val notice = connectionNotice(Link.Down("  \n", 2_000), null, secondsLeft = 2)!!
        assertNull(notice.reason)
        assertEquals("Reconnecting in 2 s", notice.text)
        assertEquals("Not connected to the host. Reconnecting.", notice.spoken)
    }

    @Test
    fun onlyTheFirstLineOfAReasonShows() {
        val notice = connectionNotice(Link.Down("timed out\n  at Socket.connect", 2_000), null, secondsLeft = 2)!!
        assertEquals("timed out", notice.reason)
    }

    @Test
    fun secondsRoundUp() {
        assertEquals(0, secondsUntil(0))
        assertEquals(1, secondsUntil(1))
        assertEquals(2, secondsUntil(2_000))
        assertEquals(3, secondsUntil(2_001))
    }
}
