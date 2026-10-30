package com.restartfu.xd.net

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class LineAssemblerTest {
    @Test
    fun splitsSeveralLinesAndDropsEmptyOnes() {
        val assembler = LineAssembler()

        assertEquals(
            listOf("""{"one":1}""", """{"two":2}"""),
            assembler.append("\n{\"one\":1}\n\n{\"two\":2}\n".encodeToByteArray()),
        )
    }

    @Test
    fun keepsUtf8BytesUntilCompleteLine() {
        val assembler = LineAssembler()
        val encoded = "before 🦴 after\n".encodeToByteArray()
        val split = encoded.indexOf(0xf0.toByte()) + 2

        assertEquals(emptyList(), assembler.append(encoded.copyOfRange(0, split)))
        assertEquals(
            listOf("before 🦴 after"),
            assembler.append(encoded.copyOfRange(split, encoded.size)),
        )
    }

    @Test
    fun rejectsInvalidUtf8OnlyWhenLineEnds() {
        val assembler = LineAssembler()

        assertEquals(emptyList(), assembler.append(byteArrayOf(0xc3.toByte())))
        assertFailsWith<InvalidUtf8Exception> {
            assembler.append(byteArrayOf(0x28, 0x0a))
        }
    }

    @Test
    fun enforcesCapAcrossChunks() {
        val assembler = LineAssembler(maxLineBytes = 4)

        assertEquals(emptyList(), assembler.append(byteArrayOf(1, 2, 3, 4)))
        assertFailsWith<LineTooLongException> {
            assembler.append(byteArrayOf(5))
        }
    }

    @Test
    fun acceptsExactlyTheCap() {
        val assembler = LineAssembler(maxLineBytes = 4)

        assertEquals(listOf("abcd"), assembler.append("abcd\n".encodeToByteArray()))
    }

    @Test
    fun resetDiscardsAPartialLine() {
        val assembler = LineAssembler()
        assembler.append("discard".encodeToByteArray())

        assembler.reset()

        assertEquals(listOf("keep"), assembler.append("keep\n".encodeToByteArray()))
    }
}
