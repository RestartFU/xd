package com.restartfu.xd.net

import com.jcraft.jsch.HostKeyRepository
import com.restartfu.xd.credentials.SshHostKey
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

class AndroidSocketTest {
    @Test
    fun outboundWritesReserveBytesUntilTheWriterFinishes() {
        val queue = OutboundWriteQueue(maxBytes = 5, maxItems = 2)

        assertTrue(queue.offer(byteArrayOf(1, 2, 3)))
        assertFalse(queue.offer(byteArrayOf(4, 5, 6)))

        val write = queue.take()
        assertFalse(queue.offer(byteArrayOf(4, 5, 6)))
        queue.complete(write)
        assertTrue(queue.offer(byteArrayOf(4, 5, 6)))
    }

    @Test
    fun outboundWritesAlsoHaveAnItemLimit() {
        val queue = OutboundWriteQueue(maxBytes = 100, maxItems = 1)

        assertTrue(queue.offer(byteArrayOf(1)))
        assertFalse(queue.offer(byteArrayOf(2)))
    }

    @Test
    fun unknownHostKeyIsCapturedAndRejectedBeforeAuthentication() {
        val repository = PinningHostKeyRepository(expected = null)
        val key = ed25519Key(1)

        assertEquals(HostKeyRepository.NOT_INCLUDED, repository.check("host", key))
        val presented = assertNotNull(repository.presented)
        assertEquals("ssh-ed25519", presented.algorithm)
        assertTrue(presented.fingerprint.startsWith("SHA256:"))
        assertTrue(key.contentEquals(presented.encoded))
    }

    @Test
    fun exactPinnedHostKeyIsAcceptedAndAnyChangeIsRejected() {
        val key = ed25519Key(1)
        val expected = SshHostKey("ssh-ed25519", key, "SHA256:expected")
        val repository = PinningHostKeyRepository(expected)

        assertEquals(HostKeyRepository.OK, repository.check("host", key.copyOf()))
        assertEquals(HostKeyRepository.CHANGED, repository.check("host", ed25519Key(2)))
    }

    @Test
    fun hostCommandRunsTheCurrentSshStdioEndpoint() {
        assertEquals(
            "exec \"\$HOME/.local/share/xd/runtime/v1/xd-host\" stdio " +
                "--data \"\$HOME/.local/share/xd\"",
            xdHostCommand("xd"),
        )
        assertEquals(
            "exec \"\$HOME/.local/share/xd-nightly/runtime/v1/xd-host\" stdio " +
                "--data \"\$HOME/.local/share/xd-nightly\"",
            xdHostCommand("xd-nightly"),
        )
        assertEquals(
            "exec \"\$HOME/.local/share/xd-dev/runtime/v1/xd-host\" stdio " +
                "--data \"\$HOME/.local/share/xd-dev\"",
            xdHostCommand("xd-dev"),
        )
    }

    @Test
    fun hostCommandRejectsAnUnrecognizedDataName() {
        assertTrue(runCatching { xdHostCommand("../other") }.isFailure)
    }

    private fun ed25519Key(fill: Byte): ByteArray =
        byteArrayOf(0, 0, 0, 11) + "ssh-ed25519".encodeToByteArray() + ByteArray(32) { fill }
}
