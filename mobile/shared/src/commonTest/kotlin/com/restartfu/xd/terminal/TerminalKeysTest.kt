package com.restartfu.xd.terminal

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertNull

class TerminalKeysTest {
    @Test
    fun enterSendsCarriageReturn() {
        // A pty in canonical mode ends a line on CR, not LF.
        assertContentEquals("\r".encodeToByteArray(), TerminalKeys.bytes(TerminalKey.ENTER))
    }

    @Test
    fun backspaceSendsDelete() {
        // Terminals have sent DEL for backspace since the VT220; readline
        // reads it as erase-previous.
        assertContentEquals(byteArrayOf(0x7F), TerminalKeys.bytes(TerminalKey.BACKSPACE))
    }

    @Test
    fun arrowsSendCursorSequences() {
        assertContentEquals(
            "\u001B[A".encodeToByteArray(),
            TerminalKeys.bytes(TerminalKey.UP),
        )
        assertContentEquals(
            "\u001B[D".encodeToByteArray(),
            TerminalKeys.bytes(TerminalKey.LEFT),
        )
    }

    @Test
    fun controlLettersBecomeControlBytes() {
        // Ctrl-C and Ctrl-D are the whole reason a phone needs a modifier.
        assertContentEquals(byteArrayOf(3), TerminalKeys.control('c')!!)
        assertContentEquals(byteArrayOf(4), TerminalKeys.control('D')!!)
        assertContentEquals(byteArrayOf(26), TerminalKeys.control('z')!!)
        assertContentEquals(byteArrayOf(1), TerminalKeys.control('a')!!)
    }

    @Test
    fun controlPunctuationIsUnderstood() {
        assertContentEquals(byteArrayOf(27), TerminalKeys.control('[')!!)
        assertContentEquals(byteArrayOf(0), TerminalKeys.control('@')!!)
    }

    @Test
    fun aCombinationWithNoControlByteIsRefused() {
        assertNull(TerminalKeys.control('1'))
        assertNull(TerminalKeys.control('-'))
    }

    @Test
    fun typedTextGoesThroughAsUtf8() {
        assertContentEquals("é".encodeToByteArray(), TerminalKeys.text("é"))
        assertContentEquals("ls -la".encodeToByteArray(), TerminalKeys.text("ls -la"))
    }

    @Test
    fun everyKeyHasAnEncoding() {
        TerminalKey.entries.forEach { key ->
            check(TerminalKeys.bytes(key).isNotEmpty()) { "$key sends nothing" }
        }
    }
}
