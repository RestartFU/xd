package com.restartfu.xd.mobile.ui

import androidx.compose.ui.graphics.Color
import com.restartfu.xd.terminal.Cell
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A terminal cell with a background of its own stays readable, by the rule
 * the desktop draws with (`light_cells_with_their_own_background_stay_readable`
 * in terminal-core): the same cells, the same colours out.
 */
class TerminalCellColoursTest {
    private val dark = terminalPalette(MinimalThemePreset.DARK)
    private val light = terminalPalette(MinimalThemePreset.LIGHT)
    private val lightInk = Color(0xFF1B1E2C)
    private val lightGround = Color(0xFFF3F4FB)
    private val darkInk = Color(0xFFF2F2F2)
    private val darkGround = Color(0xFF0B0B0B)

    private fun Cell.onLight(caret: Boolean = false) = colours(caret, lightInk, lightGround, light, light = true)
    private fun Cell.onDark(caret: Boolean = false) = colours(caret, darkInk, darkGround, dark, light = false)

    // Black on white, white on blue, and a bare red background, as a program sets them.
    private val bar = Cell('b', foreground = 0, background = 7)
    private val dir = Cell('d', foreground = 7, background = 4)
    private val bad = Cell('x', background = 1)

    @Test fun lightCellsWithTheirOwnBackgroundStayReadable() {
        for (cell in listOf(bar, dir, bad)) {
            val (ink, ground) = cell.onLight()
            assertTrue("${cell.char}: $ink on $ground", contrastRatio(ink, checkNotNull(ground)) >= MIN_CELL_CONTRAST)
        }
        // The background is the program's; only the text gives way.
        assertEquals(Color(0xFF4B5165), bar.onLight().second)
        assertEquals(Color.White, bar.onLight().first)
        assertEquals(Color.White, dir.onLight().first)
        assertEquals(Color(0xFF2A52BE), dir.onLight().second)
    }

    @Test fun cellsWithoutABackgroundOfTheirOwnAreLeftAlone() {
        // The Light set is picked to read on the terminal's ground, so text on it is never replaced.
        assertEquals(light[2] to null, Cell('p', foreground = 2).onLight())
        assertEquals(light[15] to null, Cell('w', foreground = 15).onLight())
        // No colours of its own: the terminal's.
        assertEquals(lightInk to null, Cell('a').onLight())
        assertEquals(darkInk to null, Cell('a').onDark())
    }

    @Test fun theDarkSetKeepsWhatTheProgramAskedFor() {
        assertEquals(Color(0xFF1F2329) to Color(0xFFD7DAE0), bar.onDark())
        assertEquals(Color(0xFFD7DAE0) to Color(0xFF61AFEF), dir.onDark())
        // A background with no text colour takes one that reads on it, in either set.
        assertEquals(Color(0xFF1F2329) to Color(0xFFE06C75), bad.onDark())
        assertEquals(Color.White, Cell('k', background = 0).onDark().first)
    }

    @Test fun theCaretSwapsTheCellsColoursAndStaysReadable() {
        // On a plain cell the caret is a block of the text colour.
        assertEquals(lightGround to lightInk, Cell('a').onLight(caret = true))
        assertEquals(darkGround to darkInk, Cell('a').onDark(caret = true))
        // An inverse cell under the caret is drawn the right way round again.
        assertEquals(lightInk to null, Cell('a', inverse = true).onLight(caret = true))
        // Swapped onto each other, Light's dark colours would not read: the text gives way.
        val (ink, ground) = dir.onLight(caret = true)
        assertEquals(light[7], ground)
        assertTrue(contrastRatio(ink, checkNotNull(ground)) >= MIN_CELL_CONTRAST)
    }

    @Test fun readableInkFollowsTheRealContrastOnMidTones() {
        // Grey text on a green the old brightness cut-off called dark, where white read under 3:1, and on a mid blue.
        for (ground in listOf(Color(0xFF00B000), Color(0xFF5A6EC8))) {
            val ink = checkNotNull(terminalCellInk(Color(0xFF505050), ground, light, light = true))
            assertTrue("$ink on $ground", contrastRatio(ink, ground) >= 4f)
            val bare = checkNotNull(terminalCellInk(null, ground, dark, light = false))
            assertTrue("$bare on $ground", contrastRatio(bare, ground) >= 4f)
        }
    }

    @Test fun contrastRatioMatchesTheWcagExtremes() {
        assertEquals(21f, contrastRatio(Color.Black, Color.White), 0.01f)
        assertEquals(1f, contrastRatio(Color(0xFF777777), Color(0xFF777777)), 0.001f)
        assertEquals(contrastRatio(lightInk, lightGround), contrastRatio(lightGround, lightInk))
    }
}
