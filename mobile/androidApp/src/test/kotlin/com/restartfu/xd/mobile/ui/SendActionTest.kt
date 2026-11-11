package com.restartfu.xd.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Test

/** What the composer's round button does, including mid-turn, which the screen tests cannot render. */
class SendActionTest {
    private fun action(
        working: Boolean = false,
        resumable: Boolean = false,
        quotaReached: Boolean = false,
        empty: Boolean = true,
        answerTyped: Boolean = false,
        submittingAnswer: Boolean = false,
    ) = sendAction(working, resumable, quotaReached, empty, answerTyped, submittingAnswer)

    @Test fun aRunningTurnIsStopped() = assertEquals(SendAction.STOP, action(working = true))

    @Test fun anAnswerTypedMidTurnIsSent() =
        assertEquals(SendAction.SEND_ANSWER, action(working = true, empty = false, answerTyped = true))

    /** The answers' turn is still running; a second tap must not cancel it. */
    @Test fun anAnswerOnItsWayMidTurnIsNeverTheStop() {
        assertEquals(SendAction.ANSWER_BUSY, action(working = true, empty = false, submittingAnswer = true))
        assertEquals(SendAction.ANSWER_BUSY, action(working = true, empty = true, submittingAnswer = true))
        assertEquals(SendAction.ANSWER_BUSY, action(working = true, empty = false, answerTyped = true, submittingAnswer = true))
    }

    @Test fun anAnswerOnItsWayBetweenTurnsSendsNothingElse() =
        assertEquals(SendAction.ANSWER_BUSY, action(empty = false, submittingAnswer = true))

    @Test fun aCutShortTurnResumesWhenNothingIsTyped() {
        assertEquals(SendAction.RESUME, action(resumable = true))
        assertEquals(SendAction.SEND, action(resumable = true, empty = false))
        // A chat stopped at its quota goes on from its quota ask instead.
        assertEquals(SendAction.SEND, action(resumable = true, quotaReached = true))
    }
}
