package com.restartfu.xd.credentials

internal fun testCredentials(): StoredCredentials = StoredCredentials(
    SshConnection(
        host = "host",
        port = 22,
        username = "alice",
        authentication = SshAuthentication.Password("secret"),
        hostKey = SshHostKey(
            algorithm = "ssh-ed25519",
            encoded = byteArrayOf(1, 2, 3),
            fingerprint = "SHA256:test",
        ),
    ),
)
