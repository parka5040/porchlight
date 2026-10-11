package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import kotlin.time.Instant

internal class AuditFinalClientBFix1Fixture(dispatcher: CoroutineDispatcher) {
    val now = Instant.parse("2026-10-06T12:00:00Z")
    var saved = DeviceTrust("device", "leaf", "root", "session", "2028-10-06T12:00:00Z", emptyList(), "2028-10-06T12:00:00Z", "first")
    var beforeLoad: suspend () -> Unit = {}
    var snapshot = emptySnapshot(1)
    val keys = object : DeviceKeyStore {
        override suspend fun load(): DeviceTrust { beforeLoad(); return saved }
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { saved = trust }
        override suspend fun clear() {}
        override suspend fun csr() = "csr"
        override fun certificateExpiry(pem: String) = saved.certificateExpiresAt
    }
    private val server = object : TrustTransportFactory, ApiTransport {
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = when (path.substringBefore('?')) {
            "/v1/snapshot" -> WireJson.encodeToJsonElement(snapshot).jsonObject
            "/v1/board", "/v1/agenda" -> buildJsonObject { put("items", JsonArray(emptyList())) }
            else -> JsonObject(emptyMap())
        }
    }
    val controller = TrustController(keys, server, "desktop", { now }, false, dispatcher)
    suspend fun switchMember() { saved = saved.withIdentity("second", emptyList()); controller.restore() }
}
