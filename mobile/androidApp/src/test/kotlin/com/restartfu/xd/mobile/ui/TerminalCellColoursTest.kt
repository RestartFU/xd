package com.restartfu.xd.mobile.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextDecoration
import com.restartfu.xd.terminal.Cell
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * A terminal cell with a background of its own stays readable, by the rule
 * the desktop draws with (`light_cells_with_their_own_background_stay_readable`
 * in terminal-core), in either set since round 7: the same cells, the same
 * colours out. Faint, underlined, crossed-out and inverse cells are drawn so.
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

    @Test fun theDarkSetKeepsWhatReadsAndMendsWhatDoesNot() {
        // Black on white reads, so it is what the program asked for.
        assertEquals(Color(0xFF1F2329) to Color(0xFFD7DAE0), bar.onDark())
        // White on Dark's blue is 1.7:1: the text gives way, to the set's black, as in Light.
        assertTrue(contrastRatio(Color(0xFFD7DAE0), Color(0xFF61AFEF)) < MIN_CELL_CONTRAST)
        assertEquals(Color(0xFF1F2329) to Color(0xFF61AFEF), dir.onDark())
        // A background with no text colour takes one that reads on it, in either set.
        assertEquals(Color(0xFF1F2329) to Color(0xFFE06C75), bad.onDark())
        assertEquals(Color.White, Cell('k', background = 0).onDark().first)
        for (cell in listOf(bar, dir, bad)) {
            val (ink, ground) = cell.onDark()
            assertTrue("${cell.char}: $ink on $ground", contrastRatio(ink, checkNotNull(ground)) >= MIN_CELL_CONTRAST)
        }
    }

    @Test fun darkBrightBlackReadsOnTheDarkTerminal() {
        assertEquals(Color(0xFF828997), dark[8])
        // On the terminal's own ground, as body text.
        val ratio = contrastRatio(dark[8], darkGround)
        assertTrue("bright black on the terminal is $ratio:1", ratio >= 4.5f)
        // On the set's black, which a program can set behind it, it still clears 3:1 and is kept.
        assertTrue(contrastRatio(dark[8], dark[0]) >= MIN_CELL_CONTRAST)
        assertEquals(dark[8], Cell('g', foreground = 8, background = 0).onDark().first)
    }

    @Test fun anInverseCellInTheTerminalsOwnColoursSwapsThem() {
        // No colours of its own: the terminal's ground as its text, its text colour as its ground.
        assertEquals(darkGround to darkInk, Cell('a', inverse = true).onDark())
        assertEquals(lightGround to lightInk, Cell('a', inverse = true).onLight())
        // A highlighted bar of blanks is drawn to its end, not dropped as trailing space.
        val row = listOf(Cell('a'), Cell(' ', inverse = true), Cell(' ', inverse = true))
        val drawn = row.render(caret = -1, darkInk, darkGround, dark, light = false)
        assertEquals("a  ", drawn.text)
        assertEquals(darkInk, drawn.spanStyles.last { it.end == 3 }.item.background)
    }

    @Test fun faintTextIsFainterAndStillReads() {
        for ((ink, ground) in listOf(darkInk to darkGround, lightInk to lightGround, dark[1] to darkGround)) {
            val faint = terminalFaintInk(ink, ground)
            assertTrue("$faint on $ground", contrastRatio(faint, ground) >= MIN_CELL_CONTRAST)
            assertTrue("$faint is no fainter than $ink", contrastRatio(faint, ground) < contrastRatio(ink, ground))
        }
        // Through the cell: SGR 2 on plain text.
        val (ink, _) = Cell('a', dim = true).onDark()
        assertTrue(contrastRatio(ink, darkGround) in MIN_CELL_CONTRAST..contrastRatio(darkInk, darkGround) - 1f)
        // Text that already reads at 3:1 or less is not made fainter still.
        assertEquals(Color(0xFF555555), terminalFaintInk(Color(0xFF555555), Color(0xFF333333)))
    }

    @Test fun underlinedAndCrossedOutCellsAreDrawnSo() {
        val row = listOf(Cell('u', underline = true), Cell('s', strikethrough = true), Cell('b', underline = true, strikethrough = true), Cell(' ', underline = true))
        val drawn = row.render(caret = -1, darkInk, darkGround, dark, light = false)
        assertEquals("usb ", drawn.text)
        val decorations = drawn.spanStyles.sortedBy { it.start }.map { it.item.textDecoration }
        assertEquals(
            listOf(
                TextDecoration.Underline,
                TextDecoration.LineThrough,
                TextDecoration.combine(listOf(TextDecoration.Underline, TextDecoration.LineThrough)),
                TextDecoration.Underline,
            ),
            decorations,
        )
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
