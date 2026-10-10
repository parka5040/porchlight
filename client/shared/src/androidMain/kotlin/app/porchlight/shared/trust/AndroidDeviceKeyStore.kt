package app.porchlight.shared.trust

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import app.porchlight.shared.transport.*
import java.security.*
import java.security.spec.ECGenParameterSpec
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.decodeFromString

/** Private EC key never leaves AndroidKeyStore; certificate/session metadata is AES-GCM encrypted at rest. */
class AndroidDeviceKeyStore(context: Context, private val alias: String = "porchlight.device", private val prefsName: String = "porchlight.trust") : JavaDeviceKeyStore {
    private val prefs = context.applicationContext.getSharedPreferences(prefsName, Context.MODE_PRIVATE)
    private val store get() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    private val metadataAlias = "$alias.metadata"
    override val enrollmentKeyAlias get() = alias
    private fun ensureKey() {
        if (store.containsAlias(alias)) return
        KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").apply {
            initialize(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY)
                .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                .setDigests(KeyProperties.DIGEST_NONE, KeyProperties.DIGEST_SHA256).setUserAuthenticationRequired(false).build())
        }.generateKeyPair()
    }
    override fun publicKey(): PublicKey { ensureKey(); return store.getCertificate(alias).publicKey }
    override fun privateKey(): PrivateKey { ensureKey(); return store.getKey(alias, null) as PrivateKey }
    private fun metadataKey(create: Boolean): SecretKey {
        if (!store.containsAlias(metadataAlias)) {
            if (!create) throw LocalTrustUnavailable()
            KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
                init(KeyGenParameterSpec.Builder(metadataAlias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256).setUserAuthenticationRequired(false).build())
            }.generateKey()
        }
        return store.getKey(metadataAlias, null) as SecretKey
    }
    override suspend fun csr(): String = withContext(Dispatchers.IO) { csr(publicKey(), privateKey()) }
    private fun decrypt(entry: String): String? {
        val encrypted = prefs.getString(entry, null) ?: return null
        if (!store.containsAlias(alias)) throw LocalTrustUnavailable()
        val bytes = Base64.decode(encrypted, Base64.NO_WRAP)
        require(bytes.size > 12)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, metadataKey(false), GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
        cipher.updateAAD((if (entry == "encrypted") prefsName else "$prefsName:$entry").toByteArray())
        return cipher.doFinal(bytes.copyOfRange(12, bytes.size)).toString(Charsets.UTF_8)
    }
    private fun encrypt(entry: String, text: String): String {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, metadataKey(true))
        cipher.updateAAD((if (entry == "encrypted") prefsName else "$prefsName:$entry").toByteArray())
        return Base64.encodeToString(cipher.iv + cipher.doFinal(text.toByteArray()), Base64.NO_WRAP)
    }
    override suspend fun load(): DeviceTrust? = withContext(Dispatchers.IO) {
        decrypt("encrypted")?.let { WireJson.decodeFromString<DeviceTrust>(it) }
    }
    private fun pendingEnrollments(): PendingEnrollments = try {
        val pending = decrypt("enrollment-attempt")
            ?.let { WireJson.decodeFromString<PendingEnrollments>(it) } ?: PendingEnrollments()
        check(pending.attempts.all { it.keyAlias == alias })
        pending
    } catch (e: Exception) {
        // A transient Keystore/provider error must keep the stored attempt and surface as retryable.
        if (!isDefinitivelyUnreadableAttempt(e)) throw e
        // Lost key, AEAD failure, malformed/old metadata: this attempt cannot recover anything.
        // Drop only pending metadata; enrolled trust (if any) has its separate recovery path.
        check(prefs.edit().remove("enrollment-attempt").commit()) { "Could not discard unreadable enrollment" }
        PendingEnrollments()
    }
    override suspend fun loadEnrollmentAttempt(targetBinding: String?): EnrollmentAttempt? = withContext(Dispatchers.IO) {
        pendingEnrollments().find(targetBinding)?.also { check(it.keyAlias == alias) { "Enrollment key unavailable" } }
    }
    override suspend fun saveEnrollmentAttempt(attempt: EnrollmentAttempt) = withContext(Dispatchers.IO) {
        check(attempt.keyAlias == alias && store.containsAlias(alias))
        val pending = pendingEnrollments().with(attempt)
        check(prefs.edit().putString("enrollment-attempt", encrypt("enrollment-attempt", WireJson.encodeToString(pending))).commit())
    }
    override suspend fun clearEnrollmentAttempt(targetBinding: String?) = withContext(Dispatchers.IO) {
        val remaining = pendingEnrollments().without(targetBinding)
        val edit = prefs.edit()
        if (remaining.attempts.isEmpty()) edit.remove("enrollment-attempt")
        else edit.putString("enrollment-attempt", encrypt("enrollment-attempt", WireJson.encodeToString(remaining)))
        check(edit.commit())
    }
    override suspend fun abandonEnrollment() = withContext(Dispatchers.IO) {
        check(prefs.edit().remove("enrollment-attempt").commit())
        if (!prefs.contains("encrypted")) {
            val opened = store
            if (opened.containsAlias(alias)) opened.deleteEntry(alias)
            if (opened.containsAlias(metadataAlias)) opened.deleteEntry(metadataAlias)
        }
    }
    override suspend fun save(trust: DeviceTrust, expectedPin: String?) = withContext(Dispatchers.IO) {
        validateTrust(trust, publicKey(), expectedPin)
        certificate(trust.certificate).checkValidity()
        check(prefs.edit().putString("encrypted", encrypt("encrypted", WireJson.encodeToString(trust)))
            .remove("enrollment-attempt").commit()) { "Could not commit device trust" }
    }
    override suspend fun clear() = withContext(Dispatchers.IO) {
        check(prefs.edit().clear().commit())
        val opened = store
        if (opened.containsAlias(alias)) opened.deleteEntry(alias)
        if (opened.containsAlias(metadataAlias)) opened.deleteEntry(metadataAlias)
    }
}

/** True only when the stored pending attempt can never be read again (missing key, failed authentication, malformed or foreign metadata). */
internal fun isDefinitivelyUnreadableAttempt(e: Throwable): Boolean =
    e is LocalTrustUnavailable || e is javax.crypto.AEADBadTagException || e is javax.crypto.BadPaddingException ||
        e is IllegalArgumentException || e is IllegalStateException || e is kotlinx.serialization.SerializationException
