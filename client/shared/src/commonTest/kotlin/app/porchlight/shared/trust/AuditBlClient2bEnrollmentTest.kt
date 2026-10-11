package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

internal class AuditEnrollmentKeys : DeviceKeyStore {
    var attempt: EnrollmentAttempt? = null
    var stored: DeviceTrust? = null
    var failSave = false
    override suspend fun load() = stored
    override suspend fun csr() = "fixture-csr"
    override suspend fun save(trust: DeviceTrust, expectedPin: String?) {
        if (failSave) error("Local storage unavailable")
        stored = trust; attempt = null
    }
    override suspend fun clear() { stored = null }
    override suspend fun loadEnrollmentAttempt(targetBinding: String?) = attempt
    override suspend fun saveEnrollmentAttempt(attempt: EnrollmentAttempt) { this.attempt = attempt }
    override suspend fun clearEnrollmentAttempt(targetBinding: String?) { attempt = null }
    override fun certificateExpiry(pem: String) = "2027-10-05T12:00:00Z"
}
internal class AuditEnrollmentFactory : TrustTransportFactory, ApiTransport {
    var failure: Exception? = null
    var factoryFailure: Exception? = null
    var malformed = false
    var closed = 0
    override fun enrollment(uri: EnrollUri): ApiTransport { factoryFailure?.let { throw it }; return this }
    override suspend fun authenticated(trust: DeviceTrust) = this
    override fun close() { closed++ }
    override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
        failure?.let { throw it }
        return WireJson.parseToJsonElement(if (malformed) "{}" else """{"device_id":"device","certificate":"leaf","ca_certificate":"root","session":{"token":"session","expires_at":"2028-12-01T00:00:00Z"},"endpoints":["https://lan:8443"]}""").jsonObject
    }
}
internal val auditEnrollmentUri = "porchlight://enroll?host=lan&token=fixture-token&ca=sha256:${"ab".repeat(32)}"
internal val auditEnrollmentNow = Instant.parse("2026-10-05T12:00:00Z")

class AuditBlClient2bEnrollmentTest {
    @Test fun unsavedSuccessExplainsLocalRecoveryAndKeepsExactAttempt() = runTest {
        val keys = AuditEnrollmentKeys().also { it.failSave = true }
        val factory = AuditEnrollmentFactory()
        val controller = TrustController(keys, factory, "desktop", { auditEnrollmentNow }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore()
            controller.runAction { controller.enroll(auditEnrollmentUri, "fixture-password") }!!.join()
            assertEquals("Enrollment succeeded, but device trust could not be saved locally. Your pending attempt is retained. Check local storage, then retry with the original URI and password; do not Start over.", controller.actionError.value)
            assertNotNull(keys.attempt)
            assertNull(keys.stored)
            assertEquals(TrustState.Enrollment, controller.state.value)
        } finally { controller.close() }
    }

    @Test fun cancellationAndMalformedSuccessRetainAttemptAndCloseTransport() = runTest {
        for (cancelled in listOf(true, false)) {
            val keys = AuditEnrollmentKeys()
            val factory = AuditEnrollmentFactory().also { if (cancelled) it.failure = CancellationException("Cancelled") else it.malformed = true }
            val controller = TrustController(keys, factory, "desktop", { auditEnrollmentNow }, false, StandardTestDispatcher(testScheduler))
            try {
                assertFails { controller.enroll(auditEnrollmentUri, "fixture-password") }
                val retained = keys.attempt
                assertNotNull(retained)
                assertFails { controller.enroll(auditEnrollmentUri, "fixture-password") }
                assertTrue(keys.attempt == retained, "Retry must preserve the exact attempt")
                assertNull(keys.stored)
                assertEquals(2, factory.closed)
            } finally { controller.close() }
        }
    }

    @Test fun enrollmentFactoryFailureClearsOnlyAnAttemptCreatedByThatCall() = runTest {
        for (existing in listOf(false, true)) {
            val keys = AuditEnrollmentKeys()
            val factory = AuditEnrollmentFactory().also { it.failure = IllegalStateException("Lost reply") }
            val controller = TrustController(keys, factory, "desktop", { auditEnrollmentNow }, false, StandardTestDispatcher(testScheduler))
            try {
                if (existing) assertFails { controller.enroll(auditEnrollmentUri, "fixture-password") }
                val retained = keys.attempt
                factory.factoryFailure = IllegalStateException("Factory unavailable")
                assertFails { controller.enroll(auditEnrollmentUri, "fixture-password") }
                assertTrue(keys.attempt == retained, "A provably unsent fresh attempt clears; an older ambiguous attempt stays")
                assertNull(keys.stored)
            } finally { controller.close() }
        }
    }
}
