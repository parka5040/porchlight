package app.porchlight.android.trust

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.shared.trust.*
import app.porchlight.shared.transport.*
import kotlinx.serialization.json.JsonObject
import app.porchlight.shared.transport.ClientInfo
import app.porchlight.shared.transport.OkHttpTrustFactory
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assume.assumeTrue
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import kotlin.time.Clock

/** Controller pushes an opt-in private config using adb run-as; no secrets in instrumentation arguments. */
@ControllerOnly
@RunWith(AndroidJUnit4::class)
class AndroidEnrollmentE2eTest {
    @Test fun enrollLoginMeAndLogoutOverMtls() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val file = File(context.filesDir, "c1b-e2e.json")
        val required = InstrumentationRegistry.getArguments().getString("PORCHLIGHT_E2E_REQUIRED") == "1" || System.getenv("PORCHLIGHT_E2E_REQUIRED") == "1"
        if (required) check(file.exists()) { "Required emulator e2e configuration is absent" }
        assumeTrue("Controller-only emulator enrollment config is absent", file.exists())
        val config = JSONObject(file.readText())
        val uri = config.optString("uri")
        val password = config.optString("password")
        file.delete()
        val configured = uri.isNotBlank() && password.isNotBlank()
        if (required) check(configured) { "Required emulator e2e configuration is incomplete" }
        assumeTrue("Emulator enrollment URI/password configuration is absent", configured)
        runBlocking {
            val keys = AndroidDeviceKeyStore(context, "porchlight.emulator-e2e", "porchlight-emulator-e2e")
            val real = OkHttpTrustFactory(keys, ClientInfo("0.1.0"))
            val dropReply = object : TrustTransportFactory {
                override fun enrollment(uri: EnrollUri): ApiTransport {
                    val delegate = real.enrollment(uri)
                    return object : ApiTransport {
                        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                            delegate.request(method, path, body, session, idempotencyKey, version, stepUp)
                            throw java.io.IOException("Reply lost after server write")
                        }
                        override fun close() = delegate.close()
                    }
                }
                override suspend fun authenticated(trust: DeviceTrust) = real.authenticated(trust)
            }
            val first = TrustController(keys, dropReply, "android", { Clock.System.now() }, true)
            try {
                try { first.enroll(uri, password); org.junit.Assert.fail("Reply must be lost") }
                catch (_: java.io.IOException) { /* real server committed; durable attempt is still encrypted */ }
                org.junit.Assert.assertNotNull(keys.loadEnrollmentAttempt())
            } finally { first.close() }
            // New controller AND real store: no reliance on a process-local CSR cache.
            val reopened = AndroidDeviceKeyStore(context, "porchlight.emulator-e2e", "porchlight-emulator-e2e")
            val controller = TrustController(reopened, OkHttpTrustFactory(reopened, ClientInfo("0.1.0")), "android", { Clock.System.now() }, true)
            try {
                controller.enroll(uri, password)
                org.junit.Assert.assertNull(reopened.loadEnrollmentAttempt())
                assertEquals(TrustState.PinRequired, controller.state.value)
                controller.unlockPin() // This transport probe is not an app-content PIN bypass.
                controller.refreshEndpointsAndRenew() // Calls GET /v1/me and GET /v1/endpoints over mTLS.
                controller.logout()
                assertEquals(TrustState.Login, controller.state.value)
                controller.login(password)
                controller.refreshEndpointsAndRenew()
                controller.logout()
                assertEquals(TrustState.Login, controller.state.value)
            } finally { runCatching { controller.transport().close() }; keys.clear(); file.delete() }
        }
    }
}
