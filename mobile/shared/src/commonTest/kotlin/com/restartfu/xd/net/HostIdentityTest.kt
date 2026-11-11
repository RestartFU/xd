package com.restartfu.xd.net

import com.restartfu.xd.credentials.SshAuthentication
import com.restartfu.xd.credentials.SshConnection
import com.restartfu.xd.credentials.SshHostKey
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals

class HostIdentityTest {
    private fun connection(authentication: SshAuthentication, hostKey: SshHostKey? = null) = SshConnection(
        host = "192.168.1.20",
        port = 8765,
        username = "xd",
        authentication = authentication,
        hostKey = hostKey,
    )

    @Test
    fun pairedHostsAtOneAddressAreToldApartByTheirPairedKey() {
        val a = connection(SshAuthentication.Paired("SHA256:aaa"))
        val b = connection(SshAuthentication.Paired("SHA256:bbb"))
        assertNotEquals(hostIdentity(a), hostIdentity(b))
        assertEquals("paired:SHA256:aaa", hostIdentity(a))
        // The same paired host keeps its name if its address changes.
        assertEquals(hostIdentity(a), hostIdentity(a.copy(host = "10.0.0.5")))
    }

    @Test
    fun sshHostsUseTheirPinnedKeyOrElseWhereTheyAreReached() {
        val pinned = connection(SshAuthentication.Password("x"), SshHostKey("ssh-ed25519", byteArrayOf(1), "SHA256:ccc"))
        assertEquals("key:SHA256:ccc", hostIdentity(pinned))
        assertEquals("ssh:xd@192.168.1.20:8765", hostIdentity(connection(SshAuthentication.Password("x"))))
    }
}
