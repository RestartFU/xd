package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals

class TurnTimingTest {
    @Test
    fun matchesTheDesktopAtEveryBoundary() {
        // These boundaries keep the two clients from drifting on wording.
        assertEquals("Working for 0s", TurnTiming.format("Working", 0))
        assertEquals("Worked for 59s", TurnTiming.format("Worked", 59))
        assertEquals("Worked for 1m 00s", TurnTiming.format("Worked", 60))
        assertEquals("Worked for 59m 59s", TurnTiming.format("Worked", 3599))
        assertEquals("Worked for 1h 00m", TurnTiming.format("Worked", 3600))
        assertEquals("Worked for 2h 03m", TurnTiming.format("Worked", 7380))
    }

    @Test
    fun clampsAClockThatRunsBackwards() {
        assertEquals("Working for 0s", TurnTiming.format("Working", -3))
    }

    @Test
    fun readsTheSameWithoutAVerb() {
        // Workflow cards name the run beside the count, so they drop the verb.
        assertEquals("0s", TurnTiming.duration(0))
        assertEquals("59s", TurnTiming.duration(59))
        assertEquals("1m 00s", TurnTiming.duration(60))
        assertEquals("59m 59s", TurnTiming.duration(3599))
        assertEquals("1h 00m", TurnTiming.duration(3600))
        assertEquals("0s", TurnTiming.duration(-3))
    }
}
