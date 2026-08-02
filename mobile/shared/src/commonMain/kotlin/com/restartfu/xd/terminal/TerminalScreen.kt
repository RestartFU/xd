package com.restartfu.xd.terminal

/** A character cell: what to draw and how. */
public data class Cell(
    val char: Char = ' ',
    val foreground: Int? = null,
    val background: Int? = null,
    val bold: Boolean = false,
    val inverse: Boolean = false,
)

/**
 * A terminal screen driven by raw pty bytes.
 *
 * The daemon broadcasts what the pty wrote, so a client has to interpret it:
 * the same bytes drive every attached device, and appending them as text would
 * show escape sequences instead of output.
 *
 * This implements the part of VT100/xterm that shell output actually uses --
 * cursor movement, erase, scrolling and SGR colour. Sequences beyond that are
 * consumed and ignored rather than printed, so an unsupported escape costs
 * formatting rather than turning the screen into noise. A full-screen
 * application will not render faithfully; the desktop keeps VTE for that.
 */
public class TerminalScreen(
    columns: Int = 80,
    rows: Int = 24,
) {
    public var columns: Int = columns.coerceAtLeast(1)
        private set
    public var rows: Int = rows.coerceAtLeast(1)
        private set

    private var grid = blank(this.columns, this.rows)
    private var cursorX = 0
    private var cursorY = 0
    private var style = Cell()
    private var pending = ByteArray(0)
    private var parser = Parser.GROUND
    private val sequence = StringBuilder()

    /** Where the next character lands, so a client can draw the caret. */
    public val cursorRow: Int get() = cursorY
    public val cursorColumn: Int get() = cursorX

    /** Rows top to bottom, each already [columns] cells wide. */
    public fun snapshot(): List<List<Cell>> = grid.map { it.toList() }

    public fun resize(columns: Int, rows: Int) {
        val width = columns.coerceAtLeast(1)
        val height = rows.coerceAtLeast(1)
        if (width == this.columns && height == this.rows) return

        val resized = blank(width, height)
        // Keep the bottom of the screen: that is where the prompt is.
        val keep = minOf(this.rows, height)
        for (row in 0 until keep) {
            val from = this.rows - keep + row
            val to = height - keep + row
            for (column in 0 until minOf(this.columns, width)) {
                resized[to][column] = grid[from][column]
            }
        }
        this.columns = width
        this.rows = height
        grid = resized
        cursorX = cursorX.coerceIn(0, width - 1)
        cursorY = cursorY.coerceIn(0, height - 1)
    }

    public fun write(bytes: ByteArray) {
        val joined = if (pending.isEmpty()) bytes else pending + bytes
        val complete = completeUtf8Length(joined)
        pending = joined.copyOfRange(complete, joined.size)
        if (complete == 0) return
        write(joined.copyOfRange(0, complete).decodeToString())
    }

    public fun write(text: String) {
        text.forEach { ch ->
            when (parser) {
                Parser.GROUND -> ground(ch)
                Parser.ESCAPE -> escape(ch)
                Parser.CSI -> csi(ch)
                Parser.OSC -> osc(ch)
                Parser.OSC_ESCAPE -> oscEscape(ch)
            }
        }
    }

    private fun ground(ch: Char) {
        when (ch) {
            '\u001B' -> parser = Parser.ESCAPE
            '\n' -> newline()
            '\r' -> cursorX = 0
            '\b' -> cursorX = (cursorX - 1).coerceAtLeast(0)
            '\u0007' -> Unit
            '\t' -> {
                val next = ((cursorX / 8) + 1) * 8
                cursorX = next.coerceAtMost(columns - 1)
            }
            else -> if (ch >= ' ') put(ch)
        }
    }

    private fun escape(ch: Char) {
        when (ch) {
            '[' -> {
                sequence.clear()
                parser = Parser.CSI
            }
            ']' -> {
                sequence.clear()
                parser = Parser.OSC
            }
            // Reverse index: scroll down when already at the top.
            'M' -> {
                if (cursorY == 0) scrollDown() else cursorY -= 1
                parser = Parser.GROUND
            }
            else -> parser = Parser.GROUND
        }
    }

    private fun csi(ch: Char) {
        if (ch in ' '..'?') {
            sequence.append(ch)
            return
        }
        applyCsi(ch, sequence.toString())
        sequence.clear()
        parser = Parser.GROUND
    }

    private fun osc(ch: Char) {
        // A title ends at BEL, or at ST which arrives as ESC \.
        if (ch == '\u0007') {
            parser = Parser.GROUND
            return
        }
        if (ch == '\u001B') {
            parser = Parser.OSC_ESCAPE
        }
    }

    private fun oscEscape(ch: Char) {
        // Consume the backslash in the two-byte ST terminator. If ESC was not
        // followed by one, remain inside the OSC rather than leaking the byte.
        parser = when (ch) {
            '\\' -> Parser.GROUND
            7.toChar() -> Parser.GROUND
            else -> Parser.OSC
        }
    }

    private fun applyCsi(final: Char, body: String) {
        if (body.startsWith("?")) return
        val numbers = body.split(';').map { it.toIntOrNull() }
        fun at(index: Int, fallback: Int) =
            numbers.getOrNull(index)?.takeIf { it > 0 } ?: fallback

        when (final) {
            'A' -> cursorY = (cursorY - at(0, 1)).coerceAtLeast(0)
            'B' -> cursorY = (cursorY + at(0, 1)).coerceAtMost(rows - 1)
            'C' -> cursorX = (cursorX + at(0, 1)).coerceAtMost(columns - 1)
            'D' -> cursorX = (cursorX - at(0, 1)).coerceAtLeast(0)
            'G' -> cursorX = (at(0, 1) - 1).coerceIn(0, columns - 1)
            'd' -> cursorY = (at(0, 1) - 1).coerceIn(0, rows - 1)
            'H', 'f' -> {
                cursorY = (at(0, 1) - 1).coerceIn(0, rows - 1)
                cursorX = (at(1, 1) - 1).coerceIn(0, columns - 1)
            }
            'J' -> erase(numbers.getOrNull(0) ?: 0, screen = true)
            'K' -> erase(numbers.getOrNull(0) ?: 0, screen = false)
            'm' -> style(numbers)
            else -> Unit
        }
    }

    private fun erase(mode: Int, screen: Boolean) {
        val rowsToClear = if (screen) {
            when (mode) {
                1 -> 0 until cursorY
                2, 3 -> 0 until rows
                else -> (cursorY + 1) until rows
            }
        } else {
            IntRange.EMPTY
        }
        rowsToClear.forEach { row -> grid[row] = Array(columns) { Cell() } }

        if (mode == 2 || mode == 3) {
            if (screen) {
                cursorX = 0
                cursorY = 0
            }
            if (!screen) grid[cursorY] = Array(columns) { Cell() }
            return
        }
        val columnsToClear = when (mode) {
            1 -> 0..cursorX
            else -> cursorX until columns
        }
        columnsToClear.forEach { column ->
            if (column in 0 until columns) grid[cursorY][column] = Cell()
        }
    }

    private fun style(numbers: List<Int?>) {
        if (numbers.isEmpty() || numbers.all { it == null }) {
            style = Cell()
            return
        }
        var index = 0
        while (index < numbers.size) {
            when (val code = numbers[index] ?: 0) {
                0 -> style = Cell()
                1 -> style = style.copy(bold = true)
                22 -> style = style.copy(bold = false)
                7 -> style = style.copy(inverse = true)
                27 -> style = style.copy(inverse = false)
                39 -> style = style.copy(foreground = null)
                49 -> style = style.copy(background = null)
                in 30..37 -> style = style.copy(foreground = code - 30)
                in 90..97 -> style = style.copy(foreground = code - 90 + 8)
                in 40..47 -> style = style.copy(background = code - 40)
                in 100..107 -> style = style.copy(background = code - 100 + 8)
                38, 48 -> {
                    // 256-colour and truecolour selectors carry their own
                    // arguments; consume them so they cannot be misread.
                    val foreground = code == 38
                    when (numbers.getOrNull(index + 1)) {
                        5 -> {
                            val value = numbers.getOrNull(index + 2)
                            style = if (foreground) {
                                style.copy(foreground = value)
                            } else {
                                style.copy(background = value)
                            }
                            index += 2
                        }
                        2 -> index += 4
                        else -> Unit
                    }
                }
                else -> Unit
            }
            index += 1
        }
    }

    private fun put(ch: Char) {
        if (cursorX >= columns) {
            cursorX = 0
            newline()
        }
        grid[cursorY][cursorX] = style.copy(char = ch)
        cursorX += 1
    }

    private fun newline() {
        if (cursorY >= rows - 1) scrollUp() else cursorY += 1
    }

    private fun scrollUp() {
        for (row in 0 until rows - 1) grid[row] = grid[row + 1]
        grid[rows - 1] = Array(columns) { Cell() }
    }

    private fun scrollDown() {
        for (row in rows - 1 downTo 1) grid[row] = grid[row - 1]
        grid[0] = Array(columns) { Cell() }
    }

    private fun blank(columns: Int, rows: Int): Array<Array<Cell>> =
        Array(rows) { Array(columns) { Cell() } }

    private enum class Parser { GROUND, ESCAPE, CSI, OSC, OSC_ESCAPE }

    private companion object {
        /**
         * The length of the longest prefix ending on a complete UTF-8
         * sequence. A pty write can split a multibyte character, and decoding
         * the halves separately would corrupt it.
         */
        fun completeUtf8Length(bytes: ByteArray): Int {
            var index = bytes.size
            var scanned = 0
            while (index > 0 && scanned < 4) {
                index -= 1
                scanned += 1
                val byte = bytes[index].toInt() and 0xFF
                if (byte and 0xC0 == 0x80) continue
                val needed = when {
                    byte and 0x80 == 0x00 -> 1
                    byte and 0xE0 == 0xC0 -> 2
                    byte and 0xF0 == 0xE0 -> 3
                    byte and 0xF8 == 0xF0 -> 4
                    else -> 1
                }
                return if (index + needed <= bytes.size) bytes.size else index
            }
            return bytes.size
        }
    }
}
