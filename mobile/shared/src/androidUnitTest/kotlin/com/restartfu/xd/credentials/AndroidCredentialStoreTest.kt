package com.restartfu.xd.credentials

import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertIs

class AndroidCredentialStoreTest {
    @Test
    fun passwordCredentialRecordRoundTrips() {
        val credentials = credentials(SshAuthentication.Password("secret-password"))

        val decoded = decodeCredentialRecord(encodeCredentialRecord(credentials))

        assertEquals("127.0.0.1", decoded.connection.host)
        assertEquals(2222, decoded.connection.port)
        assertEquals("danick", decoded.connection.username)
        assertEquals("SHA256:host", decoded.connection.hostKey?.fingerprint)
        assertContentEquals(byteArrayOf(1, 2, 3, 4), decoded.connection.hostKey?.encoded)
        assertEquals("secret-password", assertIs<SshAuthentication.Password>(decoded.connection.authentication).value)
    }

    @Test
    fun privateKeyCredentialRecordRoundTrips() {
        val privateKey = "-----BEGIN OPENSSH PRIVATE KEY-----\nkey\n-----END OPENSSH PRIVATE KEY-----\n".encodeToByteArray()
        val credentials = credentials(SshAuthentication.PrivateKey(privateKey, passphrase = "key-passphrase"))

        val decoded = decodeCredentialRecord(encodeCredentialRecord(credentials))
        val authentication = assertIs<SshAuthentication.PrivateKey>(decoded.connection.authentication)

        assertContentEquals(privateKey, authentication.bytes)
        assertEquals("key-passphrase", authentication.passphrase)
        assertEquals("ssh-ed25519", decoded.connection.hostKey?.algorithm)
        assertContentEquals(byteArrayOf(1, 2, 3, 4), decoded.connection.hostKey?.encoded)
    }

    private fun credentials(authentication: SshAuthentication) = StoredCredentials(
        SshConnection(
            host = "127.0.0.1",
            port = 2222,
            username = "danick",
            authentication = authentication,
            hostKey = SshHostKey(
                algorithm = "ssh-ed25519",
                encoded = byteArrayOf(1, 2, 3, 4),
                fingerprint = "SHA256:host",
            ),
        ),
    )
}
