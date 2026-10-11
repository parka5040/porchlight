package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import kotlin.time.Instant

internal class AuditFinalClientCFixture {
    val now = Instant.parse("2026-10-06T12:00:00Z")
    var saved: DeviceTrust? = DeviceTrust("device", "leaf", "root", "session", "2028-10-06T12:00:00Z", emptyList(), "2028-10-06T12:00:00Z", "first")
    var snapshot = emptySnapshot(1)
    var afterSave: suspend () -> Unit = {}
    var onStream: suspend () -> Unit = { awaitCancellation() }
    var member = "first"
    val keys = object : DeviceKeyStore {
        override suspend fun load() = saved
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { saved = trust; afterSave() }
        override suspend fun clear() { saved = null }
        override suspend fun csr() = "csr"
        override fun certificateExpiry(pem: String) = "2028-10-06T12:00:00Z"
    }
    private val server = object : TrustTransportFactory, ApiTransport {
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun stream(after: Long, session: String?, receive: suspend (StreamFrame) -> Unit) { onStream() }
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = when (path.substringBefore('?')) {
            "/v1/snapshot" -> WireJson.encodeToJsonElement(snapshot).jsonObject
            "/v1/board", "/v1/agenda" -> buildJsonObject { put("items", JsonArray(emptyList())) }
            else -> JsonObject(emptyMap())
        }
    }
    fun controller(dispatcher: CoroutineDispatcher) = TrustController(keys, server, "desktop", { now }, false, dispatcher)
}
