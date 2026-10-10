package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import org.bouncycastle.asn1.x500.X500Name
import org.bouncycastle.asn1.x509.*
import org.bouncycastle.cert.jcajce.*
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder
import java.nio.file.Files
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.cert.X509Certificate
import java.math.BigInteger
import java.util.Date
import kotlin.test.*
import kotlinx.coroutines.test.runTest

internal object TestCertificates {
    val date = Date()
    private val serial = java.util.concurrent.atomic.AtomicLong(1)
    fun key(): KeyPair = KeyPairGenerator.getInstance("EC").apply { initialize(256) }.generateKeyPair()
    fun cert(key: KeyPair, issuerKey: KeyPair = key, issuer: X509Certificate? = null, ca: Boolean = true, host: String = "localhost", expired: Boolean = false, lifetimeDays: Int = 400): X509Certificate {
        val name = X500Name(if (ca) "CN=Test CA" else "CN=$host")
        val start = Date(date.time - 86_400_000L)
        val end = Date(date.time + if (expired) -1 else lifetimeDays.toLong() * 86_400_000L)
        val builder = JcaX509v3CertificateBuilder(issuer?.let { X500Name(it.subjectX500Principal.name) } ?: name, BigInteger.valueOf(serial.getAndIncrement()), start, end, name, key.public)
        builder.addExtension(Extension.basicConstraints, true, BasicConstraints(ca))
        builder.addExtension(Extension.keyUsage, true, KeyUsage(if (ca) KeyUsage.keyCertSign or KeyUsage.cRLSign else KeyUsage.digitalSignature))
        if (!ca) {
            builder.addExtension(Extension.subjectAlternativeName, false, GeneralNames(GeneralName(if (host == "127.0.0.1") GeneralName.iPAddress else GeneralName.dNSName, host)))
            builder.addExtension(Extension.extendedKeyUsage, false, ExtendedKeyUsage(arrayOf(KeyPurposeId.id_kp_serverAuth, KeyPurposeId.id_kp_clientAuth)))
        }
        return JcaX509CertificateConverter().getCertificate(builder.build(JcaContentSignerBuilder("SHA256withECDSA").build(issuerKey.private)))
    }
}
class PlatformTrustTest {
    @Test fun missingWalletSecretWithExistingSnapshotRequiresReEnrollment() = runTest {
        val base = java.nio.file.Path.of(System.getProperty("porchlight.test.dir"))
        Files.createDirectories(base)
        val dir = Files.createTempDirectory(base, "lost-secret-")
        try {
            val wallet = FakeWallet()
            DesktopKeyStore(dir, wallet).csr()
            wallet.secret = null
            val keys = DesktopKeyStore(dir, wallet)
            assertFailsWith<LocalTrustUnavailable> { keys.load() }
            val controller = TrustController(keys, OkHttpTrustFactory(keys, ClientInfo("0.1.0")), "desktop", { kotlin.time.Clock.System.now() }, false)
            controller.restore()
            assertEquals(TrustState.LocalTrustLost(false), controller.state.value)
            controller.startReEnrollment()
            assertFalse(Files.exists(dir.resolve("device.p12")))
        } finally { dir.toFile().deleteRecursively() }
    }
    @Test fun pendingEnrollmentIsEncryptedDurableAndAtomicallyRemovedWithTrust() = runTest {
        val base = java.nio.file.Path.of(System.getProperty("porchlight.test.dir"))
        Files.createDirectories(base)
        val dir = Files.createTempDirectory(base, "pending-enrollment-")
        try {
            val wallet = FakeWallet()
            val store = DesktopKeyStore(dir, wallet) { TestCertificates.date }
            val csr = store.csr()
            val attempt = EnrollmentAttempt("target-binding", newTrustRequestKey(), csr, store.enrollmentKeyAlias)
            store.saveEnrollmentAttempt(attempt)
            val reloaded = DesktopKeyStore(dir, wallet) { TestCertificates.date }
            val pending = assertNotNull(reloaded.loadEnrollmentAttempt())
            assertEquals(csr, pending.csr)
            assertEquals(attempt.idempotencyKey, pending.idempotencyKey)
            assertEquals(attempt.keyAlias, pending.keyAlias)
            assertNull(reloaded.load())
            val other = EnrollmentAttempt("other-target", newTrustRequestKey(), reloaded.csr(), "device")
            reloaded.saveEnrollmentAttempt(other)
            val multiple = DesktopKeyStore(dir, wallet) { TestCertificates.date }
            assertEquals(csr, multiple.loadEnrollmentAttempt("target-binding")?.csr)
            assertEquals(other.csr, multiple.loadEnrollmentAttempt("other-target")?.csr)
            multiple.clearEnrollmentAttempt("other-target")
            assertNotNull(DesktopKeyStore(dir, wallet).loadEnrollmentAttempt("target-binding"))
            assertNull(DesktopKeyStore(dir, wallet).loadEnrollmentAttempt("other-target"))
            val disk = String(Files.readAllBytes(dir.resolve("device.p12")), Charsets.ISO_8859_1)
            assertFalse(disk.contains(csr))
            assertFalse(disk.contains(attempt.idempotencyKey))
            val rootKey = TestCertificates.key(); val root = TestCertificates.cert(rootKey)
            val leaf = TestCertificates.cert(KeyPair(reloaded.publicKey(), reloaded.privateKey()), rootKey, root, ca = false)
            val trust = DeviceTrust("device", pem(leaf), pem(root), "session", "2030-01-01T00:00:00Z",
                listOf("https://localhost:8443"), leaf.notAfter.toInstant().toString())
            assertFails { reloaded.save(trust, "00".repeat(32)) }
            assertNotNull(DesktopKeyStore(dir, wallet).loadEnrollmentAttempt())
            reloaded.save(trust, fingerprint(root))
            val committed = DesktopKeyStore(dir, wallet)
            assertNull(committed.loadEnrollmentAttempt())
            assertEquals("session", committed.load()?.session)
            val enrolledPublicKey = committed.publicKey().encoded
            committed.saveEnrollmentAttempt(EnrollmentAttempt("another-target", newTrustRequestKey(), committed.csr(), "device"))
            committed.abandonEnrollment()
            assertNull(committed.loadEnrollmentAttempt())
            assertEquals("session", committed.load()?.session, "Start over never clears enrolled trust")
            assertContentEquals(enrolledPublicKey, committed.publicKey().encoded)
        } finally { dir.toFile().deleteRecursively() }
    }
    @Test fun startOverDeletesUnusedDesktopKeyButKeepsWalletProtectedStorage() = runTest {
        val base = java.nio.file.Path.of(System.getProperty("porchlight.test.dir"))
        Files.createDirectories(base)
        val dir = Files.createTempDirectory(base, "unused-enrollment-")
        try {
            val wallet = FakeWallet()
            val keys = DesktopKeyStore(dir, wallet) { TestCertificates.date }
            val csr = keys.csr(); val previous = keys.publicKey().encoded
            keys.saveEnrollmentAttempt(EnrollmentAttempt("target", newTrustRequestKey(), csr, "device"))
            keys.abandonEnrollment()
            val reopened = DesktopKeyStore(dir, wallet) { TestCertificates.date }
            assertNull(reopened.loadEnrollmentAttempt())
            assertNull(reopened.load())
            assertFalse(previous.contentEquals(reopened.publicKey().encoded), "Fresh enrollment gets a new unused key")
        } finally { dir.toFile().deleteRecursively() }
    }
    @Test fun pkcs12RoundTripAndLockedWallet() = runTest {
        val base = java.nio.file.Path.of(System.getProperty("porchlight.test.dir"))
        Files.createDirectories(base)
        val dir = Files.createTempDirectory(base, "keystore-")
        try {
            val wallet = FakeWallet()
            val store = DesktopKeyStore(dir, wallet) { TestCertificates.date }
            val csr = store.csr()
            assertTrue(csr.startsWith("-----BEGIN CERTIFICATE REQUEST-----"))
            val rootKey = TestCertificates.key(); val root = TestCertificates.cert(rootKey)
            val pair = KeyPair(store.publicKey(), store.privateKey())
            val leaf = TestCertificates.cert(pair, rootKey, root, ca = false)
            val trust = DeviceTrust("device", pem(leaf), pem(root), "session", "2030-01-01T00:00:00Z", listOf("https://localhost:8443"), leaf.notAfter.toInstant().toString())
            store.save(trust, fingerprint(root))
            val reopened = DesktopKeyStore(dir, wallet) { TestCertificates.date }
            assertEquals("session", reopened.load()?.session)
            assertContentEquals(store.publicKey().encoded, reopened.publicKey().encoded)
            assertContentEquals(store.privateKey().encoded, reopened.privateKey().encoded)
            assertFalse(String(Files.readAllBytes(dir.resolve("device.p12")), Charsets.ISO_8859_1).contains("session"))
            wallet.locked = true
            assertFailsWith<WalletUnavailable> { DesktopKeyStore(dir, wallet) { TestCertificates.date }.load() }
            assertFailsWith<WalletUnavailable> { DesktopKeyStore(dir, wallet) { TestCertificates.date }.csr() }
            wallet.locked = false
            assertEquals("session", DesktopKeyStore(dir, wallet) { TestCertificates.date }.load()?.session)
            assertFails { reopened.save(trust, "00".repeat(32)) }
            assertEquals("session", DesktopKeyStore(dir, wallet) { TestCertificates.date }.load()?.session)
            reopened.clear()
            assertNull(DesktopKeyStore(dir, wallet) { TestCertificates.date }.load())
        } finally { dir.toFile().deleteRecursively() }
    }
}
internal class FakeWallet : SecretWallet {
    var locked = false
    var secret: String? = null
    override fun read(): String? { if (locked) throw WalletUnavailable(); return secret }
    override fun write(value: String) { if (locked) throw WalletUnavailable(); secret = value }
}
