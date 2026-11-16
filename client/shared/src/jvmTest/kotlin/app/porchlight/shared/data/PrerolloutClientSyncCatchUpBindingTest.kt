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
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncCatchUpBindingTest {
    private class Keys : DeviceKeyStore {
        var value: DeviceTrust? = DeviceTrust("device", "leaf", "root", "fixture-session", "2028-10-06T12:00:00Z",
            listOf(testOrigin()), "2028-10-06T12:00:00Z", memberId = "member")
        override suspend fun load() = value
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { value = trust }
        override suspend fun csr() = "csr"
        override suspend fun clear() { value = null }
        override fun certificateExpiry(pem: String) = "2028-10-06T12:00:00Z"
    }
    @Test fun coldPinLockedWorkerDiscoversSupportAndNegotiatesCatchUpFromTheDurableCursor() = runTest {
        val paths = mutableListOf<String>()
        var capability: String? = null; var after: String? = null
        val transport = KtorApiTransport(HttpClient(MockEngine { request ->
            paths += request.url.encodedPath
            when (request.url.encodedPath) {
                "/v1/me" -> respond("""{"member_id":"member","capabilities":["item-sync-v1"]}""",
                    headers = headersOf(HttpHeaders.ContentType, "application/json"))
                "/v1/stream" -> {
                    capability = request.headers["Porchlight-Capabilities"]; after = request.headers["Last-Event-ID"]
                    respond("event: ready\ndata: {\"cursor\":41}\n\nevent: caught_up\ndata: {\"cursor\":99}\n\n" +
                        "id: 100\nevent: item.changed\ndata: {\"seq\":100,\"kind\":\"item.changed\",\"entity_type\":\"item\",\"entity_id\":\"later\"}\n\n",
                        headers = headersOf(HttpHeaders.ContentType, "text/event-stream"))
                }
                else -> error("Unexpected worker read")
            }
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("0.1.8"), { "UTC" }, { "unused" }), { false })
        val factory = object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = transport
            override suspend fun authenticated(trust: DeviceTrust) = transport
        }
        val controller = TrustController(Keys(), factory, "android", { SyncBundleFixtures.now }, true, StandardTestDispatcher(testScheduler))
        val cache = memoryCache { SyncBundleFixtures.now }
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { SyncBundleFixtures.now })
        val binding = data.bindTrust(controller, backgroundScope, streamEnabled = MutableStateFlow(false),
            refreshWhenInactive = false, resumeFromCursor = true)
        try {
            controller.restore(); assertEquals(TrustState.PinRequired, controller.state.value)
            cache.install(emptySnapshot(41)); assertFalse(data.itemSync)
            // Use the same discovery + bounded shared path as Android's network worker, with the real binding source.
            val completed = withContext(Dispatchers.Default) { withBackgroundSession(controller) {
                controller.refreshSyncCapabilities()
                backgroundSafetyNet(false, false, data.targetedSync, cache.sync.value?.cursor, data.live,
                    { data.notifications.refresh() }, { data.sync.refresh() })
            } }
            assertTrue(completed); assertTrue(data.itemSync)
            assertEquals(listOf("/v1/me", "/v1/stream"), paths)
            assertEquals("item-sync-v1", capability); assertEquals("41", after)
            assertEquals(41, cache.sync.value?.cursor, "Filtered watermark is not an acknowledgement")
            assertEquals(TrustState.PinRequired, controller.state.value)
        } finally { binding.cancelAndJoin(); data.close(); controller.close(); transport.close(); runCurrent() }
    }
}
