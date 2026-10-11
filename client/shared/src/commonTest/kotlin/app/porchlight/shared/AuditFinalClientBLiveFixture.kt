package app.porchlight.shared

import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.serialization.json.JsonObject
import kotlin.time.Instant

internal class AuditFinalClientBLiveFixture(dispatcher: CoroutineDispatcher) {
    val now = Instant.parse("2026-10-06T12:00:00Z")
    private var saved = DeviceTrust("device", "leaf", "root", "session", "2028-10-06T12:00:00Z", emptyList(), "2028-10-06T12:00:00Z", "first")
    private val keys = object : DeviceKeyStore {
        override suspend fun load() = saved
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { saved = trust }
        override suspend fun clear() {}
        override suspend fun csr() = "csr"
        override fun certificateExpiry(pem: String) = saved.certificateExpiresAt
    }
    private val server = object : TrustTransportFactory, ApiTransport {
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = JsonObject(emptyMap())
    }
    val controller = TrustController(keys, server, "desktop", { now }, false, dispatcher)
    suspend fun switchMember() { saved = saved.withIdentity("second", emptyList()); controller.restore() }
}
