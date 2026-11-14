package com.restartfu.xd.mobile.ui

import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Which layout each window size gets, and how wide its reading surfaces are. */
class WindowLayoutTest {
    @Test fun aPhoneUprightGetsThePhoneLayout() {
        val layout = WindowLayout(360, 780)
        assertFalse(layout.rail)
        assertFalse(layout.tabletChat)
        assertFalse(layout.sideBySide)
    }

    @Test fun aPhoneOnItsSideKeepsTheCompactChat() {
        // Wide enough for a tablet's width class, far too short for its stacked header.
        val layout = WindowLayout(780, 360)
        assertTrue("the rail costs no height", layout.rail)
        assertFalse(layout.tabletChat)
        assertFalse(layout.sideBySide)
        // Even a very wide one: height decides the header, not width.
        assertFalse(WindowLayout(915, 412).tabletChat)
        assertFalse(WindowLayout(915, 412).sideBySide)
    }

    @Test fun anUprightTabletGetsTheTabletHeaderAndOnePane() {
        val layout = WindowLayout(600, 960)
        assertTrue(layout.rail)
        assertTrue(layout.tabletChat)
        assertFalse(layout.sideBySide)
        assertFalse(WindowLayout(839, 1200).sideBySide)
    }

    @Test fun aWideWindowPutsToolsBesideTheChat() {
        for (layout in listOf(WindowLayout(1280, 800), WindowLayout(840, 600))) {
            assertTrue(layout.rail)
            assertTrue(layout.tabletChat)
            assertTrue("$layout", layout.sideBySide)
        }
        // The height class is the boundary: 480dp is tall enough, 479dp is not.
        assertTrue(WindowLayout(840, 480).sideBySide)
        assertFalse(WindowLayout(840, 479).tabletChat)
    }

    @Test fun aReadingColumnStopsAtItsMeasure() {
        // Narrow: the edge padding alone. Wide: whatever centres 720dp.
        assertEquals(12.dp, readingInset(412.dp, 12.dp))
        assertEquals(12.dp, readingInset(744.dp, 12.dp))
        assertEquals(240.dp, readingInset(1200.dp, 12.dp))
        assertEquals(ReadingMeasure, 1200.dp - readingInset(1200.dp, 12.dp) * 2)
    }

    @Test fun theChatKeepsItsMeasureBesideAToolWhereThereIsRoom() {
        // A 1280dp window less its rail: the chat reads at its measure, the tool takes the rest.
        assertEquals(ReadingMeasure + 24.dp, chatPaneWidth(1200.dp, 12.dp))
        // Just wide enough for two panes: they split evenly.
        assertEquals(380.dp, chatPaneWidth(760.dp, 12.dp))
        // In between, the tool keeps a usable width and the chat takes what is left.
        assertEquals(600.dp, chatPaneWidth(1000.dp, 12.dp))
    }

    @Test fun aPictureZoomsBetweenFittedAndItsLimitAndStaysOnScreen() {
        val fitted = ImageZoom()
        // Fitted, a drag moves nothing and a pinch in does nothing.
        assertEquals(fitted, fitted.moved(1f, 300f, -200f, 1000, 2000))
        assertEquals(fitted, fitted.moved(0.5f, 0f, 0f, 1000, 2000))
        // Zoomed to twice the size, half the picture's width is off screen on each side at most.
        val zoomed = fitted.moved(2f, 0f, 0f, 1000, 2000)
        assertEquals(2f, zoomed.scale)
        assertEquals(ImageZoom(2f, 500f, -1000f), zoomed.moved(1f, 900f, -5000f, 1000, 2000))
        // A pinch never passes the limit, and zooming back out brings the picture home.
        assertEquals(ImageZoom.MAX_SCALE, zoomed.moved(100f, 0f, 0f, 1000, 2000).scale)
        assertEquals(fitted, zoomed.moved(1f, 400f, 400f, 1000, 2000).moved(0.1f, 0f, 0f, 1000, 2000))
        assertEquals("Fitted to the screen", fitted.label)
        assertEquals("Zoomed to 200%", zoomed.label)
    }

    @Test fun aWidePictureZoomedInCanOnlyBeDraggedWhereItIsWiderThanTheScreen() {
        // 2000 by 500 on a 1000 by 2000 screen is drawn 1000 by 250; at twice the size it is
        // 2000 by 500, so it moves 500 either way across but stays centred down the screen.
        val zoomed = ImageZoom().moved(2f, 0f, 0f, 1000, 2000, 2000, 500)
        assertEquals(ImageZoom(2f, 500f, 0f), zoomed.moved(1f, 900f, -5000f, 1000, 2000, 2000, 500))
        // A tall one is the other way round.
        assertEquals(ImageZoom(3f, 0f, 2000f), ImageZoom().moved(3f, 400f, 4000f, 1000, 2000, 100, 1000))
    }
}
