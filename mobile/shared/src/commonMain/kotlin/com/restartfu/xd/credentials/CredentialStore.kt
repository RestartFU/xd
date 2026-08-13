package com.restartfu.xd.credentials

public sealed interface SshAuthentication {
    public data class Password(val value: String) : SshAuthentication

    public data class PrivateKey(
        val bytes: ByteArray,
        val passphrase: String? = null,
    ) : SshAuthentication
}

public data class SshHostKey(
    val algorithm: String,
    val encoded: ByteArray,
    val fingerprint: String,
)

public data class SshConnection(
    val host: String,
    val port: Int,
    val username: String,
    val authentication: SshAuthentication,
    val hostKey: SshHostKey? = null,
)

public data class StoredCredentials(
    val connection: SshConnection,
)

public interface CredentialStore {
    /** Returns null when any member is absent or unreadable. */
    public suspend fun load(): StoredCredentials?

    /** Persists all members as one credential record. */
    public suspend fun save(credentials: StoredCredentials)

    public suspend fun clear()
}

internal class MemoryCredentialStore(
    initial: StoredCredentials? = null,
) : CredentialStore {
    private var credentials: StoredCredentials? = initial?.copyStored()

    override suspend fun load(): StoredCredentials? = credentials?.copyStored()

    override suspend fun save(credentials: StoredCredentials) {
        this.credentials = credentials.copyStored()
    }

    override suspend fun clear() {
        credentials = null
    }
}

internal fun StoredCredentials.copyStored(): StoredCredentials = copy(
    connection = connection.copyConnection(),
)

internal fun SshConnection.copyConnection(): SshConnection = copy(
    authentication = when (val authentication = authentication) {
        is SshAuthentication.Password -> authentication.copy()
        is SshAuthentication.PrivateKey -> authentication.copy(bytes = authentication.bytes.copyOf())
    },
    hostKey = hostKey?.copy(encoded = hostKey.encoded.copyOf()),
)
