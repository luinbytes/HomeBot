package dev.homebot.android.connection

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class AndroidKeystoreSessionStore(context: Context) : SessionStore {
    private val preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
    private val json = Json { encodeDefaults = true }

    override suspend fun load(): SessionCredentials? = withContext(Dispatchers.IO) {
        val encoded = preferences.getString(CIPHERTEXT, null) ?: return@withContext null
        runCatching {
            val envelope = json.decodeFromString<EncryptedEnvelope>(encoded)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, envelope.iv.decode()))
            val plaintext = cipher.doFinal(envelope.ciphertext.decode()).decodeToString()
            json.decodeFromString<SessionCredentialsWire>(plaintext).toDomain()
        }.getOrElse {
            preferences.edit().remove(CIPHERTEXT).apply()
            null
        }
    }

    override suspend fun save(credentials: SessionCredentials) = withContext(Dispatchers.IO) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val plaintext = json.encodeToString(SessionCredentialsWire.from(credentials)).encodeToByteArray()
        val envelope = EncryptedEnvelope(cipher.iv.encode(), cipher.doFinal(plaintext).encode())
        preferences.edit().putString(CIPHERTEXT, json.encodeToString(envelope)).commit()
        Unit
    }

    override suspend fun clear() = withContext(Dispatchers.IO) {
        preferences.edit().remove(CIPHERTEXT).commit()
        Unit
    }

    private fun key(): SecretKey {
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
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

    private fun ByteArray.encode(): String = Base64.encodeToString(this, Base64.NO_WRAP)
    private fun String.decode(): ByteArray = Base64.decode(this, Base64.NO_WRAP)

    @Serializable
    private data class EncryptedEnvelope(val iv: String, val ciphertext: String)

    @Serializable
    private data class SessionCredentialsWire(
        val endpoint: String,
        val deviceId: String,
        val deviceSession: String,
    ) {
        fun toDomain() = SessionCredentials(endpoint, deviceId, deviceSession)

        companion object {
            fun from(value: SessionCredentials) =
                SessionCredentialsWire(value.endpoint, value.deviceId, value.deviceSession)
        }
    }

    private companion object {
        const val PREFERENCES = "homebot.device_session.encrypted"
        const val CIPHERTEXT = "ciphertext"
        const val KEY_ALIAS = "homebot-device-session-v1"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
    }
}
