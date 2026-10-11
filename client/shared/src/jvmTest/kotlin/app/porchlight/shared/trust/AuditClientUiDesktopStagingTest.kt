package app.porchlight.shared.trust

import app.porchlight.shared.transport.pem
import app.porchlight.shared.transport.fingerprint
import kotlinx.coroutines.test.runTest
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*

class AuditClientUiDesktopStagingTest {
    @Test fun startupCleansCrashSnapshotsWithoutFinalSnapshot() = staged(startup = true, finalExists = false)
    @Test fun startupCleansCrashSnapshotsWithFinalSnapshot() = staged(startup = true, finalExists = true)
    @Test fun clearCleansCrashSnapshots() = staged(startup = false, finalExists = true)
    private fun staged(startup: Boolean, finalExists: Boolean) = runTest {
        val base = Path.of(System.getProperty("porchlight.test.dir"))
        Files.createDirectories(base)
        val dir = Files.createTempDirectory(base, "audit-staging-")
        try {
            val wallet = object : SecretWallet {
                private var value: String? = null
                override fun read() = value
                override fun write(value: String) { this.value = value }
            }
            val keys = DesktopKeyStore(dir, wallet)
            keys.csr()
            val rootKey = TestCertificates.key(); val root = TestCertificates.cert(rootKey)
            val leaf = TestCertificates.cert(java.security.KeyPair(keys.publicKey(), keys.privateKey()), rootKey, root, ca = false)
            val trust = DeviceTrust("device", pem(leaf), pem(root), "fixture-session", "2030-01-01T00:00:00Z", listOf("https://example.invalid"), leaf.notAfter.toInstant().toString())
            keys.save(trust, fingerprint(root))
            val staged = dir.resolve(".device-crash-fixture.p12")
            Files.copy(dir.resolve("device.p12"), staged)
            val unrelated = Files.writeString(dir.resolve("keep.p12"), "unrelated")
            if (!finalExists) Files.delete(dir.resolve("device.p12"))
            if (startup) assertEquals(if (finalExists) trust.session else null, DesktopKeyStore(dir, wallet).load()?.session) else keys.clear()
            assertFalse(Files.exists(staged), "crashed PKCS12 staging snapshot must be removed")
            assertTrue(Files.exists(unrelated), "cleanup is narrowly scoped")
            if (!startup) assertFalse(Files.exists(dir.resolve("device.p12")))
        } finally { dir.toFile().deleteRecursively() }
    }
}
