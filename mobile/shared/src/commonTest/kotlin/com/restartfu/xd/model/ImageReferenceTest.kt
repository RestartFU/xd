package com.restartfu.xd.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ImageReferenceTest {
    @Test
    fun findsAMarkerOnItsOwnLine() {
        val parts = ImageReference.parts("look\n[image: /pastes/a.png]")

        assertEquals(
            listOf(MessagePart.Prose("look"), MessagePart.Image("/pastes/a.png")),
            parts,
        )
    }

    @Test
    fun keepsProseAndImagesInOrder() {
        val parts = ImageReference.parts(
            "before\n[image: /a.png]\nbetween\n[image: /b.png]\nafter",
        )

        assertEquals(
            listOf(
                MessagePart.Prose("before"),
                MessagePart.Image("/a.png"),
                MessagePart.Prose("between"),
                MessagePart.Image("/b.png"),
                MessagePart.Prose("after"),
            ),
            parts,
        )
    }

    @Test
    fun aMarkerMidSentenceStaysProse() {
        // Matching the desktop: the whole line, and nothing else on it.
        val text = "see [image: /a.png] there"

        assertEquals(listOf(MessagePart.Prose(text)), ImageReference.parts(text))
        assertFalse(ImageReference.hasImage(text))
    }

    @Test
    fun toleratesCarriageReturns() {
        val parts = ImageReference.parts("hi\r\n[image: /a.png]\r")

        assertEquals(
            listOf(MessagePart.Prose("hi"), MessagePart.Image("/a.png")),
            parts,
        )
    }

    @Test
    fun aMessageWithoutMarkersIsOneRunOfProse() {
        val parts = ImageReference.parts("just words\nacross lines")

        assertEquals(listOf(MessagePart.Prose("just words\nacross lines")), parts)
        assertFalse(ImageReference.hasImage("just words"))
    }

    @Test
    fun reportsWhetherAMessageCarriesAnImage() {
        assertTrue(ImageReference.hasImage("a\n[image: /a.png]\nb"))
        assertFalse(ImageReference.hasImage("[image: ]"))
    }
}
