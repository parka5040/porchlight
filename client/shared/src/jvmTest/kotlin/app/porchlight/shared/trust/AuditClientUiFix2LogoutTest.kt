package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import java.nio.file.Files
import java.nio.file.Path
import java.security.KeyPair
import java.util.Date
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class AuditClientUiFix2LogoutTest {
    @Test fun expiredPendingLogoutRestoresAndReEnrolls() = expiredLogout(false)
    @Test fun expiredPendingLogoutReEnrollsAfterCleanupFailure() = expiredLogout(true)

    private fun expiredLogout(failCleanup: Boolean) = runTest {
        val base = Path.of(System.getProperty("porchlight.test.dir"))
        Files.createDirectories(base)
        val dir = Files.createTempDirectory(base, "audit-expired-logout-")
        val wallet = FakeWallet()
        try {
            val enrolling = DesktopKeyStore(dir, wallet) { Date(TestCertificates.date.time - 60_000) }
            val rootKey = TestCertificates.key(); val root = TestCertificates.cert(rootKey)
            val leaf = TestCertificates.cert(KeyPair(enrolling.publicKey(), enrolling.privateKey()), rootKey, root, ca = false, expired = true)
            val saved = DeviceTrust("device", pem(leaf), pem(root), "fixture-session", "2030-01-01T00:00:00Z",
                listOf("https://example.invalid"), leaf.notAfter.toInstant().toString(), logoutPending = true)
            enrolling.save(saved)
            val keys = DesktopKeyStore(dir, wallet)
            assertFailsWith<java.security.cert.CertificateExpiredException> { keys.save(saved) }
            var fail = failCleanup
            var cleared = false
            val controller = TrustController(keys, object : TrustTransportFactory {
                override fun enrollment(uri: EnrollUri): ApiTransport = error("Offline")
                override suspend fun authenticated(trust: DeviceTrust): ApiTransport = error("Offline")
            }, "desktop", { Instant.fromEpochMilliseconds(System.currentTimeMillis()) }, false)
            controller.identityEnded = { if (fail) error("Storage unavailable"); cleared = true }
            try {
                controller.restore()
                if (failCleanup) {
                    assertEquals(TrustState.LocalTrustLost(false), controller.state.value)
                    assertTrue(DesktopKeyStore(dir, wallet).load()!!.logoutPending)
                    fail = false
                } else {
                    assertEquals(TrustState.ReEnroll, controller.state.value, "expired logout must reach re-enrollment")
                    assertTrue(cleared)
                    assertNull(DesktopKeyStore(dir, wallet).load()?.session)
                }
                controller.startReEnrollment()
                assertTrue(cleared, "re-enrollment must complete pending durable cache cleanup")
                assertEquals(TrustState.Enrollment, controller.state.value)
                assertNull(DesktopKeyStore(dir, wallet).load())
            } finally { controller.close() }
        } finally { dir.toFile().deleteRecursively() }
    }
}
