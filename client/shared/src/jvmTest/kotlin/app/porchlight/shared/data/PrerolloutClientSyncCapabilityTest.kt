package app.porchlight.shared.data

import app.porchlight.shared.live.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncCapabilityTest {
    private val now = SyncBundleFixtures.now
    private class Keys : DeviceKeyStore {
        var value: DeviceTrust? = DeviceTrust("device", "leaf", "root", "fixture-session", "2028-10-06T12:00:00Z",
            listOf(testOrigin()), "2028-10-06T12:00:00Z", memberId = "member")
        override suspend fun load() = value
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
        override suspend fun csr() = "csr"
        override suspend fun clear() { value = null }
        override fun certificateExpiry(pem: String) = "2028-10-06T12:00:00Z"
    }
    private class Stop : Exception()

    @Test fun meAdvertisementAloneControlsStreamHeaderAndAbsenceRestoresLegacy() = runTest {
        var capabilities: JsonArray? = null
        val headers = mutableListOf<String?>()
        val transport = KtorApiTransport(HttpClient(MockEngine { request ->
            when (request.url.encodedPath) {
                "/v1/me" -> respond(buildJsonObject {
                    put("member_id", "member"); put("session_expires_at", "2028-10-06T12:00:00Z")
                    capabilities?.let { put("capabilities", it) }
                }.toString(), headers = headersOf(HttpHeaders.ContentType, "application/json"))
                "/v1/endpoints" -> respond("""{"endpoints":["${testOrigin()}"]}""", headers = headersOf(HttpHeaders.ContentType, "application/json"))
                "/v1/stream" -> {
                    headers += request.headers["Porchlight-Capabilities"]
                    respond("event: ready\ndata: {\"cursor\":41}\n\n", headers = headersOf(HttpHeaders.ContentType, "text/event-stream"))
                }
                "/v1/auth/logout" -> respond("{}", headers = headersOf(HttpHeaders.ContentType, "application/json"))
                else -> error("Unexpected fixture route")
            }
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("0.1.8"), { "UTC" }, { "unused" }), { false })
        val factory = object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = transport
            override suspend fun authenticated(trust: DeviceTrust) = transport
        }
        val controller = TrustController(Keys(), factory, "desktop", { now }, false, StandardTestDispatcher(testScheduler))
        try {
            controller.restore()
            for (advertised in listOf(false, true, false)) {
                capabilities = if (advertised) JsonArray(listOf(JsonPrimitive("future"), JsonPrimitive("item-sync-v1"))) else null
                controller.refreshEndpointsAndRenew()
                assertEquals(advertised, controller.itemSync)
                assertFailsWith<Stop> { controller.transport().stream(41, controller.sessionToken(), controller.itemSync) {
                    assertEquals(StreamFrame.Ready, it); throw Stop()
                } }
            }
            assertEquals(listOf(null, "item-sync-v1", null), headers)
            capabilities = JsonArray(listOf(JsonPrimitive("item-sync-v1")))
            controller.refreshEndpointsAndRenew(); assertTrue(controller.itemSync)
            controller.logout(); assertFalse(controller.itemSync)
        } finally { controller.close(); transport.close() }
    }

    @Test fun liveBindingExposesCapabilityAndReconnectsWhenAdvertisementChanges() = runTest {
        var advertised = false
        val connected = kotlinx.coroutines.channels.Channel<Boolean>(kotlinx.coroutines.channels.Channel.UNLIMITED)
        val factory = object : TrustTransportFactory, ApiTransport {
            override fun enrollment(uri: EnrollUri) = this
            override suspend fun authenticated(trust: DeviceTrust) = this
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
                "/v1/me" -> buildJsonObject { put("member_id", "member"); if (advertised) put("capabilities", JsonArray(listOf(JsonPrimitive("item-sync-v1")))) }
                "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(listOf(JsonPrimitive(testOrigin())))) }
                "/v1/snapshot" -> WireJson.encodeToJsonElement(emptySnapshot(41)).jsonObject
                "/v1/board", "/v1/agenda" -> buildJsonObject { put("items", JsonArray(emptyList())); put("next_cursor", JsonNull) }
                else -> error("Unexpected fixture route")
            }
            override suspend fun stream(after: Long, session: String?, itemSync: Boolean, receive: suspend (StreamFrame) -> Unit) {
                connected.send(itemSync); receive(StreamFrame.Ready); awaitCancellation()
            }
        }
        val controller = TrustController(Keys(), factory, "desktop", { now }, false, StandardTestDispatcher(testScheduler))
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val enabled = MutableStateFlow(true)
        val binding = data.bindTrust(controller, backgroundScope, streamEnabled = enabled, resumeFromCursor = true)
        try {
            controller.restore(); assertFalse(connected.receive()); assertFalse(data.itemSync)
            advertised = true; controller.refreshEndpointsAndRenew()
            assertTrue(connected.receive()); assertTrue(data.itemSync)
            advertised = false; controller.refreshEndpointsAndRenew()
            assertFalse(connected.receive()); assertFalse(data.itemSync)
        } finally { binding.cancelAndJoin(); controller.close(); data.close() }
    }
}
