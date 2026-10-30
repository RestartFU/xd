package com.restartfu.xd.terminal

import kotlin.test.Test
import kotlin.test.assertEquals

class TerminalScreenTest {
    @Test
    fun printsAndWrapsText() {
        val screen = TerminalScreen(columns = 5, rows = 3)

        screen.write("abcdefg")

        assertEquals("abcde", screen.row(0))
        assertEquals("fg", screen.row(1).trimEnd())
    }

    @Test
    fun carriageReturnOverwritesTheLine() {
        val screen = TerminalScreen(columns = 10, rows = 2)

        // A progress bar redraws this way; appending would show both.
        screen.write("50%\r100%")

        assertEquals("100%", screen.row(0).trimEnd())
    }

    @Test
    fun backspaceMovesBackWithoutErasing() {
        val screen = TerminalScreen(columns = 10, rows = 2)

        screen.write("abc\bX")

        assertEquals("abX", screen.row(0).trimEnd())
    }

    @Test
    fun scrollsWhenOutputPassesTheLastRow() {
        val screen = TerminalScreen(columns = 4, rows = 2)

        // A pty applies ONLCR, so real output carries CR LF. A bare LF only
        // moves down, which is what a real terminal does too.
        screen.write("a\r\nb\r\nc")

        assertEquals("b", screen.row(0).trimEnd())
        assertEquals("c", screen.row(1).trimEnd())
    }

    @Test
    fun cursorAddressingPlacesText() {
        val screen = TerminalScreen(columns = 6, rows = 3)

        screen.write("\u001B[2;3Hxy")

        assertEquals("  xy", screen.row(1).trimEnd())
    }

    @Test
    fun eraseClearsTheScreenAndHomesTheCursor() {
        val screen = TerminalScreen(columns = 4, rows = 2)
        screen.write("ab\ncd")

        screen.write("\u001B[2J")
        screen.write("z")

        assertEquals("z", screen.row(0).trimEnd())
        assertEquals("", screen.row(1).trimEnd())
    }

    @Test
    fun eraseToEndOfLineClearsTheRest() {
        val screen = TerminalScreen(columns = 8, rows = 1)
        screen.write("abcdef")

        screen.write("\u001B[3G\u001B[K")

        assertEquals("ab", screen.row(0).trimEnd())
    }

    @Test
    fun sgrSetsAndResetsColour() {
        val screen = TerminalScreen(columns = 8, rows = 1)

        screen.write("\u001B[31;1mR\u001B[0mn")

        val cells = screen.snapshot()[0]
        assertEquals(1, cells[0].foreground)
        assertEquals(true, cells[0].bold)
        assertEquals(null, cells[1].foreground)
        assertEquals(false, cells[1].bold)
    }

    @Test
    fun brightAnd256ColourAreUnderstood() {
        val screen = TerminalScreen(columns = 8, rows = 1)

        screen.write("\u001B[91ma\u001B[38;5;200mb")

        val cells = screen.snapshot()[0]
        assertEquals(9, cells[0].foreground)
        assertEquals(200, cells[1].foreground)
    }

    @Test
    fun unsupportedSequencesAreSwallowedNotPrinted() {
        val screen = TerminalScreen(columns = 12, rows = 1)

        // Bracketed paste and a window title must not reach the screen.
        screen.write("\u001B[?2004ha\u001B]0;title\u0007b")

        assertEquals("ab", screen.row(0).trimEnd())
    }

    @Test
    fun consumesStringTerminatorWithoutPrintingItsBackslash() {
        val screen = TerminalScreen(columns = 12, rows = 1)
        val escape = 27.toChar()

        // OSC titles may be split between ESC and the backslash in ST.
        screen.write("${escape}]0;title${escape}")
        screen.write("${92.toChar()}prompt")

        assertEquals("prompt", screen.row(0).trimEnd())
    }

    @Test
    fun aSplitMultibyteCharacterSurvivesTheBoundary() {
        val screen = TerminalScreen(columns = 4, rows = 1)
        val bytes = "é".encodeToByteArray()

        // The pty can flush half a character; decoding each half alone would
        // corrupt it.
        screen.write(bytes.copyOfRange(0, 1))
        screen.write(bytes.copyOfRange(1, bytes.size))

        assertEquals("é", screen.row(0).trimEnd())
    }

    @Test
    fun resizeKeepsTheBottomOfTheScreen() {
        val screen = TerminalScreen(columns = 4, rows = 3)
        screen.write("a\r\nb\r\nc")

        screen.resize(4, 2)

        assertEquals("b", screen.row(0).trimEnd())
        assertEquals("c", screen.row(1).trimEnd())
        assertEquals(1, screen.cursorRow)
    }

    @Test
    fun growingTheScreenMovesTheCursorWithItsPrompt() {
        val screen = TerminalScreen(columns = 10, rows = 2)
        screen.write("prompt$ ")

        screen.resize(10, 5)
        screen.write("x")

        assertEquals("prompt$ x", screen.row(3).trimEnd())
        assertEquals("", screen.row(0).trimEnd())
        assertEquals(3, screen.cursorRow)
    }

    @Test
    fun shrinkingTheScreenTranslatesACursorInTheRetainedRows() {
        val screen = TerminalScreen(columns = 8, rows = 4)
        screen.write("\u001B[3;1Hprompt")

        screen.resize(8, 2)
        screen.write("$")

        assertEquals("prompt$", screen.row(0).trimEnd())
        assertEquals(0, screen.cursorRow)
    }

    private fun TerminalScreen.row(index: Int): String =
        snapshot()[index].joinToString("") { it.char.toString() }
}
