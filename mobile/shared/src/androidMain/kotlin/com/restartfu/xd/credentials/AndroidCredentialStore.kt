package com.restartfu.xd.credentials

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import com.restartfu.xd.protocol.WireJson
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * Stores one encrypted credential record. One preference key means token and
 * certificate cannot be observed in a partially-updated state.
 */
public class AndroidCredentialStore(
    context: Context,
) : CredentialStore {
    private val preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    override fun load(): StoredCredentials? {
        val record = preferences.getString(RECORD, null) ?: return null
        return runCatching {
            val separator = record.indexOf(':')
            require(separator > 0 && separator < record.lastIndex)
            val iv = Base64.decode(record.substring(0, separator), Base64.NO_WRAP)
            val ciphertext = Base64.decode(record.substring(separator + 1), Base64.NO_WRAP)
            val cipher = Cipher.getInstance(CIPHER)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, iv))
            decode(cipher.doFinal(ciphertext).decodeToString())
        }.getOrNull()
    }

    override fun save(credentials: StoredCredentials) {
        require(credentials.host.isNotBlank())
        require(credentials.port in 1..65535)
        require(credentials.token.isNotBlank())
        require(credentials.certificateDer.isNotEmpty())

        val plain = buildJsonObject {
            put("host", credentials.host)
            put("port", credentials.port)
            put("token", credentials.token)
            put(
                "certificate",
                Base64.encodeToString(credentials.certificateDer, Base64.NO_WRAP),
            )
        }.toString().encodeToByteArray()
        val cipher = Cipher.getInstance(CIPHER)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val record = Base64.encodeToString(cipher.iv, Base64.NO_WRAP) +
            ":" +
            Base64.encodeToString(cipher.doFinal(plain), Base64.NO_WRAP)
        check(preferences.edit().putString(RECORD, record).commit()) {
            "Could not persist remote credentials"
        }
    }

    override fun clear() {
        check(preferences.edit().remove(RECORD).commit()) {
            "Could not clear remote credentials"
        }
    }

    private fun decode(value: String): StoredCredentials {
        val objectValue = WireJson.parseToJsonElement(value).jsonObject
        val host = objectValue.jsonObjectValue("host")
        val port = objectValue["port"]?.jsonPrimitive?.intOrNull
            ?: error("Missing credential port")
        val token = objectValue.jsonObjectValue("token")
        val certificate = Base64.decode(
            objectValue.jsonObjectValue("certificate"),
            Base64.NO_WRAP,
        )
        require(host.isNotBlank() && port in 1..65535)
        require(token.isNotBlank() && certificate.isNotEmpty())
        return StoredCredentials(host, port, token, certificate)
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }

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
        return generator.generateKey()
    }

    private fun JsonObject.jsonObjectValue(name: String): String =
        this[name]?.jsonPrimitive?.contentOrNull
            ?: error("Missing credential $name")

    private companion object {
        const val PREFERENCES = "xd-remote-credentials"
        const val RECORD = "encrypted-record-v1"
        const val KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "xd-remote-credentials-v1"
        const val CIPHER = "AES/GCM/NoPadding"
        const val TAG_BITS = 128
    }
}
