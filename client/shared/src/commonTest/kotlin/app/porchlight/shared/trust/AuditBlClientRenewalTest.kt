package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Instant

@OptIn(ExperimentalCoroutinesApi::class)
class AuditBlClientRenewalTest {
    private val start = Instant.parse("2026-10-06T12:00:00Z")
    private class Keys(var value: DeviceTrust) : DeviceKeyStore {
        override suspend fun load() = value
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
        override suspend fun csr() = "csr"
        override suspend fun clear() {}
        override fun certificateExpiry(pem: String) = "2027-10-06T12:00:00Z"
    }
    private class Server : TrustTransportFactory, ApiTransport {
        var meCalls = 0
        var gate: CompletableDeferred<Unit>? = null
        var failRenewal = false
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path) {
            "/v1/me" -> { meCalls++; gate?.await(); jsonBody("member_id" to "member", "session_expires_at" to "2028-10-06T12:00:00Z") }
            "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(listOf(JsonPrimitive("https://fixture.invalid")))) }
            "/v1/auth/login" -> buildJsonObject { put("session", jsonBody("token" to "new-session", "expires_at" to "2028-10-06T12:00:00Z")) }
            "/v1/auth/logout" -> buildJsonObject {}
            "/v1/devices/self/certificate" -> { if (failRenewal) throw ConnectFailure(); jsonBody("certificate" to "renewed") }
            else -> error("Unexpected route")
        }
    }
    private fun keys(days: Int) = Keys(DeviceTrust("device", "leaf", "root", "session", "2028-10-06T12:00:00Z",
        listOf("https://fixture.invalid"), (start + days.days).toString()))

    @Test fun loginRearmsDailyDeadlineWithoutWaitingForTomorrow() = runTest {
        val server = Server()
        val controller = TrustController(keys(60), server, "desktop", { start + testScheduler.currentTime.milliseconds }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            assertEquals(1, server.meCalls)
            controller.onProblem(ApiProblem("session-expired", 401, "Expired"))
            controller.login("fixture password"); runCurrent()
            assertEquals(2, server.meCalls, "A replacement login must re-arm the renewal obligation")
        } finally { controller.close(); runCurrent() }
    }

    @Test fun fencedRefreshCannotDelayNewLoginForADay() = runTest {
        val server = Server().apply { gate = CompletableDeferred() }
        val controller = TrustController(keys(60), server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            assertEquals(1, server.meCalls)
            controller.onProblem(ApiProblem("session-expired", 401, "Expired"))
            controller.login("fixture password")
            server.gate!!.complete(Unit); runCurrent()
            assertEquals(2, server.meCalls, "A fenced old response cannot schedule the new identity's deadline")
        } finally { server.gate?.complete(Unit); controller.close(); runCurrent() }
    }

    @Test fun logoutClearsIdentityOwnedExpiryWarning() = runTest {
        val server = Server().apply { failRenewal = true }
        val controller = TrustController(keys(6), server, "desktop", { start }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            assertNotNull(controller.renewalWarning.value)
            controller.logout()
            assertNull(controller.renewalWarning.value, "An ended identity cannot retain its certificate warning")
        } finally { controller.close(); runCurrent() }
    }
}
