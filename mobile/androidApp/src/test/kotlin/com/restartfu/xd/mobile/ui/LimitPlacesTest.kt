package com.restartfu.xd.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class LimitPlacesTest {
    private val matches = (1..13).map { "feat/part-$it" }
    private val all = listOf("main") + matches

    @Test fun theLimitLeavesTheRestToTheFilter() {
        val (shown, hidden) = limitPlaces(matches, all, limit = 12) { false }
        assertEquals(matches.take(12), shown)
        assertEquals(1, hidden)
    }

    @Test fun aChosenRowOutsideTheFilterDoesNotHideACount() {
        // main does not match the filter but stays listed; one match is still left out.
        val (shown, hidden) = limitPlaces(matches, all, limit = 12) { it == "main" }
        assertEquals(matches.take(12) + "main", shown)
        assertEquals(1, hidden)
    }

    @Test fun aChosenMatchPastTheLimitIsShownAndNotCounted() {
        val (shown, hidden) = limitPlaces(matches, all, limit = 12) { it == "feat/part-13" }
        assertEquals(matches, shown)
        assertEquals(0, hidden)
    }

    @Test fun nothingChosenAndFewMatchesHidesNothing() {
        val (shown, hidden) = limitPlaces(matches.take(3), all, limit = 12) { false }
        assertEquals(matches.take(3), shown)
        assertEquals(0, hidden)
    }
}
