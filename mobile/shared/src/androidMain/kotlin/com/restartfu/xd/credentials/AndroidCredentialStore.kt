package com.restartfu.xd.credentials

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import com.restartfu.xd.protocol.WireJson
import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * Stores one encrypted SSH credential record. One preference key means the
 * authentication secret and pinned host key update atomically.
 */
public class AndroidCredentialStore(
    context: Context,
) : CredentialStore {
    private val preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    override suspend fun load(): StoredCredentials? = withContext(Dispatchers.IO) {
        val record = preferences.getString(RECORD, null) ?: run {
            preferences.edit().remove(LEGACY_RECORD).apply()
            return@withContext null
        }
        runCatching {
            val separator = record.indexOf(':')
            require(separator > 0 && separator < record.lastIndex)
            val iv = Base64.getDecoder().decode(record.substring(0, separator))
            val ciphertext = Base64.getDecoder().decode(record.substring(separator + 1))
            val cipher = Cipher.getInstance(CIPHER)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, iv))
            decodeCredentialRecord(cipher.doFinal(ciphertext).decodeToString())
        }.getOrNull()
    }

    override suspend fun save(credentials: StoredCredentials): Unit = withContext(Dispatchers.IO) {
        val connection = credentials.connection
        require(connection.host.isNotBlank())
        require(connection.port in 1..65535)
        require(connection.username.isNotBlank())
        val hostKey = requireNotNull(connection.hostKey)
        require(hostKey.algorithm.isNotBlank() && hostKey.encoded.isNotEmpty())
        require(hostKey.fingerprint.isNotBlank())

        val plain = encodeCredentialRecord(credentials).encodeToByteArray()
        val cipher = Cipher.getInstance(CIPHER)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val record = Base64.getEncoder().withoutPadding().encodeToString(cipher.iv) +
            ":" +
            Base64.getEncoder().withoutPadding().encodeToString(cipher.doFinal(plain))
        check(preferences.edit().putString(RECORD, record).commit()) {
            "Could not persist remote credentials"
        }
    }

    override suspend fun clear(): Unit = withContext(Dispatchers.IO) {
        check(preferences.edit().remove(RECORD).remove(LEGACY_RECORD).commit()) {
            "Could not clear remote credentials"
        }
    }

    private fun key(): SecretKey = synchronized(KEY_LOCK) {
        val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey) ?: run {
            val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
            generator.init(
                KeyGenParameterSpec.Builder(
                    KEY_ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build(),
            )
            generator.generateKey()
        }
    }

    private companion object {
        const val PREFERENCES = "xd-remote-credentials"
        const val RECORD = "encrypted-record-v2"
        const val LEGACY_RECORD = "encrypted-record-v1"
        const val KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "xd-remote-credentials-v1"
        const val CIPHER = "AES/GCM/NoPadding"
        const val TAG_BITS = 128
        val KEY_LOCK = Any()
    }
}

internal fun encodeCredentialRecord(credentials: StoredCredentials): String {
    val connection = credentials.connection
    val hostKey = requireNotNull(connection.hostKey)
    return buildJsonObject {
        put("host", connection.host)
        put("port", connection.port)
        put("username", connection.username)
        when (val authentication = connection.authentication) {
            is SshAuthentication.Password -> {
                require(authentication.value.isNotEmpty())
                put("authentication", "password")
                put("password", authentication.value)
            }
            is SshAuthentication.PrivateKey -> {
                require(authentication.bytes.isNotEmpty())
                put("authentication", "private-key")
                put("privateKey", Base64.getEncoder().withoutPadding().encodeToString(authentication.bytes))
                authentication.passphrase?.let { put("passphrase", it) }
            }
        }
        put("hostKeyAlgorithm", hostKey.algorithm)
        put("hostKey", Base64.getEncoder().withoutPadding().encodeToString(hostKey.encoded))
        put("hostKeyFingerprint", hostKey.fingerprint)
    }.toString()
}

internal fun decodeCredentialRecord(value: String): StoredCredentials {
    val objectValue = WireJson.parseToJsonElement(value).jsonObject
    val host = objectValue.stringValue("host")
    val port = objectValue["port"]?.jsonPrimitive?.intOrNull
        ?: error("Missing credential port")
    val username = objectValue.stringValue("username")
    val authentication = when (objectValue.stringValue("authentication")) {
        "password" -> SshAuthentication.Password(objectValue.stringValue("password"))
        "private-key" -> SshAuthentication.PrivateKey(
            bytes = Base64.getDecoder().decode(objectValue.stringValue("privateKey")),
            passphrase = objectValue["passphrase"]?.jsonPrimitive?.contentOrNull,
        )
        else -> error("Unknown SSH authentication type")
    }
    val hostKey = SshHostKey(
        algorithm = objectValue.stringValue("hostKeyAlgorithm"),
        encoded = Base64.getDecoder().decode(objectValue.stringValue("hostKey")),
        fingerprint = objectValue.stringValue("hostKeyFingerprint"),
    )
    require(host.isNotBlank() && port in 1..65535)
    require(username.isNotBlank() && hostKey.encoded.isNotEmpty())
    return StoredCredentials(
        SshConnection(host, port, username, authentication, hostKey),
    )
}

private fun JsonObject.stringValue(name: String): String =
    this[name]?.jsonPrimitive?.contentOrNull
        ?: error("Missing credential $name")
