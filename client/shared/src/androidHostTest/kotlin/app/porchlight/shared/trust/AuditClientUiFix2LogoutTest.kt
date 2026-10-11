package app.porchlight.shared.trust

import app.porchlight.shared.settings.PrivateMetadataStore
import app.porchlight.shared.transport.*
import java.io.InputStream
import java.io.OutputStream
import java.math.BigInteger
import java.nio.file.Files
import java.nio.file.Path
import java.security.*
import java.security.cert.Certificate
import java.security.cert.CertificateExpiredException
import java.util.*
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.encodeToString
import org.bouncycastle.asn1.x500.X500Name
import org.bouncycastle.asn1.x509.*
import org.bouncycastle.cert.jcajce.*
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder
import kotlin.test.*
import kotlin.time.Instant

/** Exercises the real encrypted Android metadata store; only AndroidKeyStore hardware is supplied by JCA fixtures. */
class AuditClientUiFix2LogoutTest {
    @Test fun expiredPendingLogoutRestoresAndReEnrolls() = expiredLogout(false)
    @Test fun expiredPendingLogoutReEnrollsAfterCleanupFailure() = expiredLogout(true)

    private fun expiredLogout(failCleanup: Boolean) = runTest {
        val base = Path.of("..", "..", "var", "test-android")
        Files.createDirectories(base)
        val dir = Files.createTempDirectory(base, "audit-android-logout-")
        val provider = object : Provider("AuditAndroidKeyStore", 1.0, "Host trust fixture") {
            init { put("KeyStore.AndroidKeyStore", AuditLogoutKeyStoreSpi::class.java.name) }
        }
        Security.addProvider(provider)
        try {
            val pair = KeyPairGenerator.getInstance("EC").apply { initialize(256) }.generateKeyPair()
            val name = X500Name("CN=Test device")
            val builder = JcaX509v3CertificateBuilder(name, BigInteger.ONE, Date(System.currentTimeMillis() - 86_400_000),
                Date(System.currentTimeMillis() - 60_000), name, pair.public)
            builder.addExtension(Extension.basicConstraints, true, BasicConstraints(true))
            builder.addExtension(Extension.extendedKeyUsage, false, ExtendedKeyUsage(KeyPurposeId.id_kp_clientAuth))
            val leaf = JcaX509CertificateConverter().getCertificate(builder.build(JcaContentSignerBuilder("SHA256withECDSA").build(pair.private)))
            val aes = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
            AuditLogoutKeyStoreSpi.entries["fixture-device"] = pair.private
            AuditLogoutKeyStoreSpi.entries["fixture-device.metadata"] = aes
            AuditLogoutKeyStoreSpi.certificate = leaf
            val saved = DeviceTrust("device", pem(leaf), pem(leaf), "fixture-session", "2030-01-01T00:00:00Z",
                listOf("https://example.invalid"), leaf.notAfter.toInstant().toString(), logoutPending = true)
            fun metadata() = PrivateMetadataStore(dir.resolve("trust.json").toFile(), { emptyMap<String, String>() }, {})
            // A snapshot committed before shutdown, now expired on process restart.
            val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
                init(Cipher.ENCRYPT_MODE, aes); updateAAD("fixture-trust".toByteArray())
            }
            metadata().edit().putString("encrypted", Base64.getEncoder().encodeToString(cipher.iv + cipher.doFinal(WireJson.encodeToString(saved).toByteArray()))).commit()
            fun keys() = AndroidDeviceKeyStore("fixture-device", "fixture-trust", ::metadata)
            assertFailsWith<CertificateExpiredException> { keys().save(saved) }
            var fail = failCleanup
            var cleared = false
            val controller = TrustController(keys(), object : TrustTransportFactory {
                override fun enrollment(uri: EnrollUri): ApiTransport = error("Offline")
                override suspend fun authenticated(trust: DeviceTrust): ApiTransport = error("Offline")
            }, "android", { Instant.fromEpochMilliseconds(System.currentTimeMillis()) }, false)
            controller.identityEnded = { if (fail) error("Storage unavailable"); cleared = true }
            try {
                controller.restore()
                if (failCleanup) {
                    assertEquals(TrustState.LocalTrustLost(true), controller.state.value)
                    assertTrue(keys().load()!!.logoutPending)
                    fail = false
                } else {
                    assertEquals(TrustState.ReEnroll, controller.state.value, "expired logout must reach re-enrollment")
                    assertTrue(cleared)
                    assertNull(keys().load()?.session)
                }
                controller.startReEnrollment()
                assertTrue(cleared, "re-enrollment must complete pending durable cache cleanup")
                assertEquals(TrustState.Enrollment, controller.state.value)
                assertNull(keys().load())
            } finally { controller.close() }
        } finally {
            Security.removeProvider(provider.name)
            AuditLogoutKeyStoreSpi.entries.clear(); AuditLogoutKeyStoreSpi.certificate = null
            dir.toFile().deleteRecursively()
        }
    }
}

class AuditLogoutKeyStoreSpi : KeyStoreSpi() {
    companion object {
        val entries = mutableMapOf<String, Key>()
        var certificate: Certificate? = null
    }
    override fun engineGetKey(alias: String, password: CharArray?): Key? = entries[alias]
    override fun engineGetCertificate(alias: String): Certificate? = certificate
    override fun engineGetCertificateChain(alias: String): Array<Certificate> = arrayOf(checkNotNull(certificate))
    override fun engineGetCreationDate(alias: String) = Date(0)
    override fun engineContainsAlias(alias: String) = alias in entries
    override fun engineDeleteEntry(alias: String) { entries.remove(alias) }
    override fun engineAliases(): Enumeration<String> = Collections.enumeration(entries.keys)
    override fun engineSize() = entries.size
    override fun engineIsKeyEntry(alias: String) = alias in entries
    override fun engineIsCertificateEntry(alias: String) = false
    override fun engineGetCertificateAlias(cert: Certificate): String? = null
    override fun engineLoad(stream: InputStream?, password: CharArray?) = Unit
    override fun engineStore(stream: OutputStream?, password: CharArray?) = error("Unused")
    override fun engineSetKeyEntry(alias: String, key: Key, password: CharArray?, chain: Array<out Certificate>?) = error("Unused")
    override fun engineSetKeyEntry(alias: String, key: ByteArray, chain: Array<out Certificate>?) = error("Unused")
    override fun engineSetCertificateEntry(alias: String, cert: Certificate) = error("Unused")
}
