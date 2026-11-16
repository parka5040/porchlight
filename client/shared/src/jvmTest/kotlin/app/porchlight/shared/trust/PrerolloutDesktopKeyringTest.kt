package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlinx.serialization.json.*
import java.nio.file.*
import java.security.KeyPair
import kotlin.test.*
import kotlin.time.Instant

class PrerolloutDesktopKeyringTest {
    @Test fun enrolledSecretServiceIdentityRecoversProviderWithoutFileFallback() = runBlocking {
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "late-keyring-")
        val bus = FakeSecretService()
        fun keys() = DesktopKeyStore(dir, DesktopKeyBackends.system(dir, null, bus, wait = {})) { TestCertificates.date }
        var controller: TrustController? = null
        try {
            val initial = keys()
            initial.csr()
            val caKey = TestCertificates.key()
            val ca = TestCertificates.cert(caKey)
            val leaf = TestCertificates.cert(KeyPair(initial.publicKey(), initial.privateKey()), caKey, ca, ca = false)
            initial.save(DeviceTrust("device", pem(leaf), pem(ca), "session", "2030-01-01T00:00:00Z", emptyList(), initial.certificateExpiry(pem(leaf))), null)
            bus.running = false
            val transport = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = JsonObject(emptyMap())
            }
            val factory = object : TrustTransportFactory {
                override fun enrollment(uri: EnrollUri) = transport
                override suspend fun authenticated(trust: DeviceTrust) = transport
            }
            val restored = TrustController(keys(), factory, "desktop", { Instant.fromEpochMilliseconds(TestCertificates.date.time) }, false)
            controller = restored
            restored.restore()
            assertEquals(TrustState.WalletLocked, restored.state.value)
            bus.running = true
            withTimeout(5_000) { restored.state.first { it == TrustState.Ready } }
            assertFalse(Files.exists(dir.resolve(KEY_DIRECTORY).resolve(KEY_FILE_SECRET)))
            assertEquals("secret-service", Files.readString(dir.resolve(KEY_DIRECTORY).resolve(KEY_BACKEND_RECORD)))
            assertEquals(0, bus.prompts)
        } finally { controller?.close(); dir.toFile().deleteRecursively() }
        Unit
    }
}
