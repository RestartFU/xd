package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class LinkedPullRequestTest {
    @Test
    fun aLinkNamesItsNumberRepositoryAndState() {
        val merged = LinkedPullRequest("https://github.com/restartfu/xd/pull/212", "merged")
        assertEquals(212L, merged.number)
        assertEquals("restartfu/xd", merged.repository)
        assertTrue(merged.merged)
        assertFalse(merged.closed)
        val unknown = LinkedPullRequest("https://github.example.com/acme/web/pull/7/")
        assertEquals(7L, unknown.number)
        assertEquals("acme/web", unknown.repository)
        assertFalse(unknown.merged || unknown.closed)
        assertNull(pullRequestRepository("https://github.com/acme"))
        assertNull(pullRequestRepository("https://github.com/acme/web/issues/7"))
        assertNull(pullRequestNumber("https://github.com/acme/web"))
    }

    @Test
    fun pastedTextIsSplitIntoOneLinkPerWord() {
        assertEquals(
            listOf("https://github.com/o/r/pull/1", "https://github.com/o/r/pull/2", "nope"),
            pastedPullRequestLinks(" https://github.com/o/r/pull/1,\nhttps://github.com/o/r/pull/2  nope https://github.com/o/r/pull/1 "),
        )
        assertEquals(emptyList(), pastedPullRequestLinks("  \n "))
    }
}
