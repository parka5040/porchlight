package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.JsonObject
import kotlin.test.*
import kotlin.time.Duration.Companion.minutes
import kotlin.time.Duration.Companion.seconds
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Instant

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientBExpiryTest {
    private val start = Instant.parse("2026-10-06T12:00:00Z")
    private class Keys(val expiry: Instant) : DeviceKeyStore {
        private var value = DeviceTrust("device", "leaf", "root", "session", "2028-10-06T12:00:00Z", emptyList(), expiry.toString())
        override suspend fun load() = value
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
        override suspend fun csr() = "csr"
        override suspend fun clear() {}
        override fun certificateExpiry(pem: String) = expiry.toString()
    }
    private class Offline : TrustTransportFactory, ApiTransport {
        var calls = 0
        var gate: CompletableDeferred<Unit>? = null
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
            calls++; gate?.await()
            // A failed TLS reconnect says nothing about whether the server or client certificate expired.
            throw RequestNotSent()
        }
    }
    @Test fun renewalWaitCannotSleepPastStoredLeafExpiry() = runTest {
        val controller = TrustController(Keys(start + 10.seconds), Offline(), "desktop",
            { start + testScheduler.currentTime.milliseconds }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            advanceTimeBy(10_000); runCurrent()
            assertEquals(TrustState.ReEnroll, controller.state.value)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun foregroundWithoutRenewalLoopAlsoRechecksTheStoredLeaf() = runTest {
        var clock = start
        val controller = TrustController(Keys(start + 1.minutes), Offline(), "desktop", { clock }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); clock = start + 1.minutes
            controller.onForeground(); runCurrent()
            assertEquals(TrustState.ReEnroll, controller.state.value)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun foregroundChecksLeafExpiryEvenDuringRenewalBackoff() = runTest {
        var clock = start
        val server = Offline()
        val controller = TrustController(Keys(start + 1.minutes), server, "desktop", { clock }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            assertEquals(1, server.calls)
            assertNotNull(controller.renewalWarning.value)
            clock = start + 1.minutes
            controller.onForeground(); runCurrent()
            assertEquals(TrustState.ReEnroll, controller.state.value, "Expired stored leaf must close a running session during backoff")
            assertNull(controller.renewalWarning.value)
            assertEquals(1, controller.sessionEpoch.value)
        } finally { controller.close(); runCurrent() }
    }
    @Test fun failedReconnectAcrossExpiryClosesGateWithoutRestart() = runTest {
        var clock = start
        val server = Offline().apply { gate = CompletableDeferred() }
        val controller = TrustController(Keys(start + 1.minutes), server, "android", { clock }, true, StandardTestDispatcher(testScheduler))
        try {
            controller.restore(); controller.refreshWhenReady(); runCurrent()
            clock = start + 1.minutes
            server.gate!!.complete(Unit); runCurrent()
            assertEquals(TrustState.ReEnroll, controller.state.value, "Failed TLS reconnect after leaf expiry must expose re-enrollment")
        } finally { server.gate?.complete(Unit); controller.close(); runCurrent() }
    }
    @Test fun renewalSnapshotRejectsExpiredLeafButNotServerTlsFailure() = runTest {
        var clock = start
        val server = Offline()
        val controller = TrustController(Keys(start + 1.minutes), server, "desktop", { clock }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore()
            assertFailsWith<RequestNotSent> { controller.refreshEndpointsAndRenew() }
            assertEquals(TrustState.Ready, controller.state.value, "Generic TLS failure with a valid leaf is not identity-end")
            clock = start + 1.minutes
            controller.refreshEndpointsAndRenew()
            assertEquals(TrustState.ReEnroll, controller.state.value)
            assertEquals(1, server.calls, "Expired leaf must be rejected before another handshake")
        } finally { controller.close(); runCurrent() }
    }
}
