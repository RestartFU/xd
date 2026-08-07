package com.restartfu.xd.voice

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class WavTest {
    @Test
    fun writesTheHeaderTheDaemonValidates() {
        // The Rust daemon checks every one of these fields before it will read
        // a recording, so they are a contract rather than a detail.
        val wav = Wav.fromPcm16(ByteArray(4))

        assertEquals("RIFF", wav.ascii(0, 4))
        assertEquals(36 + 4, wav.u32(4))
        assertEquals("WAVEfmt ", wav.ascii(8, 8))
        assertEquals(16, wav.u32(16))
        assertEquals(1, wav.u16(20))
        assertEquals(1, wav.u16(22))
        assertEquals(16_000, wav.u32(24))
        assertEquals(32_000, wav.u32(28))
        assertEquals(2, wav.u16(32))
        assertEquals(16, wav.u16(34))
        assertEquals("data", wav.ascii(36, 4))
        assertEquals(4, wav.u32(40))
    }

    @Test
    fun copiesSamplesThroughUntouched() {
        val pcm = byteArrayOf(0x01, 0x02, 0x7F.toByte(), 0x80.toByte())
        val wav = Wav.fromPcm16(pcm)

        assertEquals(Wav.HEADER_BYTES + pcm.size, wav.size)
        assertContentEquals(pcm, wav.copyOfRange(Wav.HEADER_BYTES, wav.size))
    }

    @Test
    fun refusesAudioThatIsNotWholeSamples() {
        assertFailsWith<IllegalArgumentException> { Wav.fromPcm16(ByteArray(3)) }
        assertFailsWith<IllegalArgumentException> { Wav.fromPcm16(ByteArray(0)) }
    }

    private fun ByteArray.ascii(offset: Int, length: Int): String =
        decodeToString(offset, offset + length)

    private fun ByteArray.u16(offset: Int): Int =
        (this[offset].toInt() and 0xFF) or ((this[offset + 1].toInt() and 0xFF) shl 8)

    private fun ByteArray.u32(offset: Int): Int =
        (0 until 4).sumOf { (this[offset + it].toInt() and 0xFF) shl (it * 8) }
}
