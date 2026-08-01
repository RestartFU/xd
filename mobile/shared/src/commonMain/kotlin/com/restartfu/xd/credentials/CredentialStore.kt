package com.restartfu.xd.credentials

public data class StoredCredentials(
    val host: String,
    val port: Int,
    val token: String,
    val certificateDer: ByteArray,
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
    certificateDer = certificateDer.copyOf(),
)
