package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import org.junit.Assume.assumeTrue
import java.nio.file.Files
import java.nio.file.Path
import java.util.Base64
import java.security.SecureRandom
import kotlin.test.*
import kotlin.time.Clock

class DesktopE2eTest {
    /** No secrets in assertions, exceptions, command lines or logs. Only explicitly opted-in real dev stack traffic. */
    @Test fun e2e() {
        val uri = System.getenv("PORCHLIGHT_E2E_URI")
        val password = System.getenv("PORCHLIGHT_E2E_PASSWORD")
        val configured = !uri.isNullOrBlank() && !password.isNullOrBlank()
        if (System.getenv("PORCHLIGHT_E2E_REQUIRED") == "1") check(configured) { "Required desktop e2e configuration is absent" }
        assumeTrue("Desktop e2e requires private enrollment URI/password environment", configured)
        runBlocking {
            val base = Path.of(System.getProperty("porchlight.test.dir")); Files.createDirectories(base)
            val directory = Files.createTempDirectory(base, "desktop-e2e-")
            val keys = DesktopKeyStore(directory, DesktopSecretWallet("app.porchlight.test", "desktop-e2e"))
            val factory = OkHttpTrustFactory(keys, ClientInfo("0.1.0"))
            val controller = TrustController(keys, factory, "desktop", { Clock.System.now() }, false)
            try {
                controller.enroll(checkNotNull(uri), checkNotNull(password))
                controller.logout()
                controller.login(password)
                controller.refreshEndpointsAndRenew()
                val before = checkNotNull(keys.load())
                // R22: never edit immutable history. The real server rejects fresh-certificate renewal.
                val problem = assertFailsWith<ApiProblem> { controller.refreshEndpointsAndRenew(forceRenewal = true) }
                assertEquals(409, problem.status)
                assertEquals("renewal-too-early", problem.type)
                assertEquals(before.certificate, keys.load()?.certificate)
                controller.restore()
                assertTrue(controller.transport().request("GET", "/v1/me", session = controller.sessionToken()).containsKey("member_id"))
                controller.logout()
                assertEquals(TrustState.Login, controller.state.value)
            } finally { runCatching { controller.transport().close() }; keys.clear(); directory.toFile().deleteRecursively() }
    }
    }
    @Test fun secretServiceRuntime() {
        val configured = System.getenv("PORCHLIGHT_KEYRING_TEST") == "1"
        if (System.getenv("PORCHLIGHT_E2E_REQUIRED") == "1") check(configured) { "Required wallet probe configuration is absent" }
        assumeTrue("Controller must opt into the real session wallet test", configured)
        val account = "runtime-probe"
        val value = Base64.getUrlEncoder().withoutPadding().encodeToString(ByteArray(32).also { SecureRandom().nextBytes(it) })
        val wallet = DesktopSecretWallet("app.porchlight.test", account)
        wallet.write(value)
        assertTrue(wallet.read() == value)
        assertTrue(DirectSecretService("app.porchlight.test", account).read() == value)
        // Also exercise the dbus-java 5 adapter directly, not just whichever java-keyring backend works.
        val direct = DirectSecretService("app.porchlight.test", "direct-runtime-probe")
        direct.write(value)
        assertTrue(direct.read() == value)
    }
}
