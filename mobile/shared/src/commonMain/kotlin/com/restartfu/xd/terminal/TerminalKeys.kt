package com.restartfu.xd.terminal

/** A key a soft keyboard does not offer but a shell needs. */
public enum class TerminalKey {
    ENTER,
    BACKSPACE,
    TAB,
    ESCAPE,
    UP,
    DOWN,
    RIGHT,
    LEFT,
    HOME,
    END,
    PAGE_UP,
    PAGE_DOWN,
    DELETE,
}

/**
 * What to write to the pty for a keypress.
 *
 * A terminal sends keystrokes as they happen rather than lines on submit, so
 * these are the bytes the shell expects to read. Shared because an iOS client
 * needs exactly the same encoding.
 */
public object TerminalKeys {
    public fun bytes(key: TerminalKey): ByteArray = when (key) {
        // Carriage return, not line feed: that is what a pty in canonical
        // mode treats as the end of a line.
        TerminalKey.ENTER -> "\r"
        // DEL rather than BS. Terminals have sent DEL for the backspace key
        // since the VT220, and readline reads it as erase-previous.
        TerminalKey.BACKSPACE -> "\u007F"
        TerminalKey.TAB -> "\t"
        TerminalKey.ESCAPE -> "\u001B"
        TerminalKey.UP -> "\u001B[A"
        TerminalKey.DOWN -> "\u001B[B"
        TerminalKey.RIGHT -> "\u001B[C"
        TerminalKey.LEFT -> "\u001B[D"
        TerminalKey.HOME -> "\u001B[H"
        TerminalKey.END -> "\u001B[F"
        TerminalKey.PAGE_UP -> "\u001B[5~"
        TerminalKey.PAGE_DOWN -> "\u001B[6~"
        TerminalKey.DELETE -> "\u001B[3~"
    }.encodeToByteArray()

    /**
     * The control byte for Ctrl held with [letter], or null when that
     * combination has none.
     *
     * Ctrl-C and Ctrl-D are the whole reason a phone terminal needs a
     * modifier: without them there is no way to stop or end anything.
     */
    public fun control(letter: Char): ByteArray? {
        val upper = letter.uppercaseChar()
        return when (upper) {
            in 'A'..'Z' -> byteArrayOf((upper - 'A' + 1).toByte())
            '@' -> byteArrayOf(0)
            '[' -> byteArrayOf(27)
            '\\' -> byteArrayOf(28)
            ']' -> byteArrayOf(29)
            '^' -> byteArrayOf(30)
            '_' -> byteArrayOf(31)
            ' ' -> byteArrayOf(0)
            else -> null
        }
    }

    public fun text(value: String): ByteArray = value.encodeToByteArray()
}
