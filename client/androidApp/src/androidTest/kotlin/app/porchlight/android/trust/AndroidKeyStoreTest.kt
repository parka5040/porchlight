package app.porchlight.android.trust

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.shared.trust.*
import java.security.Signature
import java.security.MessageDigest
import java.util.Base64
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import android.security.keystore.KeyInfo
import app.porchlight.shared.settings.privateMetadata
import java.security.KeyFactory

@RunWith(AndroidJUnit4::class)
class AndroidKeyStoreTest {
    @Test fun nonExportableUnauthBoundP256AndSignedCsr() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val keys = AndroidDeviceKeyStore(context, "porchlight.instrumented-key", "instrumented-trust")
        try {
            val csr = keys.csr()
            assertTrue(verifyCsr(csr, keys.publicKey()))
            // Conscrypt's CertificateVerify upcall signs an already-hashed digest.
            val digest = MessageDigest.getInstance("SHA-256").digest("TLS CertificateVerify test".toByteArray())
            val signature = Signature.getInstance("NONEwithECDSA").run {
                initSign(keys.privateKey()); update(digest); sign()
            }
            assertTrue(Signature.getInstance("NONEwithECDSA").run {
                initVerify(keys.publicKey()); update(digest); verify(signature)
            })
            assertNull(keys.privateKey().encoded)
            val info = KeyFactory.getInstance("EC", "AndroidKeyStore").getKeySpec(keys.privateKey(), KeyInfo::class.java)
            assertEquals(256, info.keySize)
            assertFalse(info.isUserAuthenticationRequired)
            assertTrue(verifyCsr(keys.csr(), keys.publicKey()))
        } finally { keys.clear() }
    }
    @Test fun transferredMetadataWithoutKeystoreMapsToReEnrollment() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val keys = AndroidDeviceKeyStore(context, "porchlight.transfer-test", "transfer-test")
        keys.clear()
        try {
            // Metadata lives in the same private store production reads (no-backup file, migrated from legacy prefs).
            privateMetadata(context, "transfer-test").edit()
                .putString("encrypted", android.util.Base64.encodeToString(ByteArray(32), android.util.Base64.NO_WRAP)).commit()
            assertTrue(runCatching { keys.load() }.exceptionOrNull() is LocalTrustUnavailable)
            val controller = TrustController(keys, app.porchlight.shared.transport.OkHttpTrustFactory(keys,
                app.porchlight.shared.transport.ClientInfo("0.1.0")), "android", { kotlin.time.Clock.System.now() }, true)
            controller.restore()
            assertEquals(TrustState.LocalTrustLost(true), controller.state.value)
            controller.startReEnrollment()
            assertNull(keys.load())
        } finally { keys.clear() }
    }
    @Test fun pendingEnrollmentSurvivesStoreReopenWithoutPlaintextOrKeyExport() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val alias = "porchlight.pending-reopen"; val prefsName = "pending-reopen"
        val keys = AndroidDeviceKeyStore(context, alias, prefsName)
        keys.clear()
        try {
            val csr = keys.csr()
            val attempt = EnrollmentAttempt("target", "test-idempotency-key", csr, alias)
            keys.saveEnrollmentAttempt(attempt)
            val encrypted = privateMetadata(context, prefsName).getString("enrollment-attempt", null)!!
            assertFalse(encrypted.contains(csr))
            assertFalse(encrypted.contains(attempt.idempotencyKey))
            val reopened = AndroidDeviceKeyStore(context, alias, prefsName)
            val recovered = reopened.loadEnrollmentAttempt("target")!!
            assertEquals(csr, recovered.csr)
            assertEquals(attempt.idempotencyKey, recovered.idempotencyKey)
            assertEquals(alias, recovered.keyAlias)
            assertTrue(verifyCsr(recovered.csr, reopened.publicKey()))
            assertNull(reopened.privateKey().encoded)
            reopened.abandonEnrollment()
            assertNull(reopened.loadEnrollmentAttempt())
            val store = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            assertFalse(store.containsAlias(alias))
        } finally { keys.clear() }
    }
    @Test fun unreadableOrWrongAliasAttemptIsDroppedAndFreshEnrollmentCanBeSaved() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val alias = "porchlight.pending-corrupt"; val prefsName = "pending-corrupt"
        val keys = AndroidDeviceKeyStore(context, alias, prefsName)
        val prefs = privateMetadata(context, prefsName)
        for (failure in listOf("device", "metadata", "ciphertext", "wrong-alias")) {
            keys.clear()
            try {
                keys.saveEnrollmentAttempt(EnrollmentAttempt("target", "request-key", keys.csr(), alias))
                val store = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
                when (failure) {
                    "device" -> store.deleteEntry(alias)
                    "metadata" -> store.deleteEntry("$alias.metadata")
                    "ciphertext" -> prefs.edit().putString("enrollment-attempt", "invalid").commit()
                    "wrong-alias" -> {
                        // Valid AEAD but metadata refers to a different private key.
                        val cipher = javax.crypto.Cipher.getInstance("AES/GCM/NoPadding")
                        cipher.init(javax.crypto.Cipher.ENCRYPT_MODE, store.getKey("$alias.metadata", null))
                        cipher.updateAAD("$prefsName:enrollment-attempt".toByteArray())
                        val text = """{"attempts":[{"targetBinding":"target","idempotencyKey":"key","csr":"csr","keyAlias":"wrong"}]}"""
                        prefs.edit().putString("enrollment-attempt", android.util.Base64.encodeToString(
                            cipher.iv + cipher.doFinal(text.toByteArray()), android.util.Base64.NO_WRAP)).commit()
                    }
                }
                val reopened = AndroidDeviceKeyStore(context, alias, prefsName)
                assertNull(reopened.loadEnrollmentAttempt("target"))
                assertFalse(prefs.contains("enrollment-attempt"))
                reopened.saveEnrollmentAttempt(EnrollmentAttempt("fresh-target", "fresh-key", reopened.csr(), alias))
                assertNotNull(reopened.loadEnrollmentAttempt("fresh-target"))
            } finally { keys.clear() }
        }
    }
    @Test fun legacyPreferencesFromAnUpgradedInstallMigrateAndDecryptOnFirstOpen() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val alias = "porchlight.legacy-migration"
        // A name no store has opened in this process: the first open must migrate what 0.1.6 left in SharedPreferences.
        val prefsName = "legacy-migration-${System.nanoTime()}"
        val seed = AndroidDeviceKeyStore(context, alias, "legacy-seed")
        seed.clear()
        try {
            seed.saveEnrollmentAttempt(EnrollmentAttempt("seed", "seed-key", seed.csr(), alias)) // device + metadata keys
            val store = java.security.KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            val cipher = javax.crypto.Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(javax.crypto.Cipher.ENCRYPT_MODE, store.getKey("$alias.metadata", null))
            cipher.updateAAD("$prefsName:enrollment-attempt".toByteArray())
            val text = """{"attempts":[{"targetBinding":"target","idempotencyKey":"legacy-key","csr":"legacy-csr","keyAlias":"$alias"}]}"""
            val legacy = context.getSharedPreferences(prefsName, android.content.Context.MODE_PRIVATE)
            assertTrue(legacy.edit().putString("enrollment-attempt", android.util.Base64.encodeToString(
                cipher.iv + cipher.doFinal(text.toByteArray()), android.util.Base64.NO_WRAP)).commit())
            val upgraded = AndroidDeviceKeyStore(context, alias, prefsName)
            assertEquals("legacy-key", upgraded.loadEnrollmentAttempt("target")?.idempotencyKey)
            assertTrue(java.io.File(context.noBackupFilesDir, "$prefsName.json").exists())
            assertTrue(legacy.all.isEmpty())
        } finally {
            seed.clear()
            java.io.File(context.noBackupFilesDir, "$prefsName.json").delete()
            context.deleteSharedPreferences(prefsName)
        }
    }
    private fun verifyCsr(pem: String, public: java.security.PublicKey): Boolean {
        val der = Base64.getMimeDecoder().decode(pem.substringAfter("-----BEGIN CERTIFICATE REQUEST-----").substringBefore("-----END CERTIFICATE REQUEST-----"))
        // Minimal DER extraction is test-only; verify the signed request-info bytes with JCA.
        var index = 0
        fun length(): Int {
            val first = der[index++].toInt() and 255
            if (first < 128) return first
            var size = 0
            repeat(first and 127) { size = (size shl 8) or (der[index++].toInt() and 255) }
            return size
        }
        assertEquals(0x30, der[index++].toInt()); length()
        val start = index
        assertEquals(0x30, der[index++].toInt()); val infoSize = length()
        index += infoSize
        val requestInfo = der.copyOfRange(start, index)
        assertEquals(0x30, der[index++].toInt()); val algorithmSize = length(); index += algorithmSize
        assertEquals(0x03, der[index++].toInt()); val signatureSize = length()
        assertEquals(0, der[index++].toInt())
        val signature = der.copyOfRange(index, index + signatureSize - 1)
        return Signature.getInstance("SHA256withECDSA").run { initVerify(public); update(requestInfo); verify(signature) }
    }
}
