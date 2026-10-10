package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class AppTrustHolderTest {
    @Test fun holderKeepsKeysControllerAndLostReplyAttemptAcrossConsumers() = runBlocking {
        var constructions = 0
        var csrs = 0
        var saved: DeviceTrust? = null
        val entered = CompletableDeferred<Unit>()
        val reply = CompletableDeferred<Unit>()
        val seenCsrs = mutableListOf<String>()
        val keys = object : DeviceKeyStore {
            var attempt: EnrollmentAttempt? = null
            override suspend fun loadEnrollmentAttempt(targetBinding: String?) = attempt?.takeIf { targetBinding == null || it.targetBinding == targetBinding }
            override suspend fun saveEnrollmentAttempt(attempt: EnrollmentAttempt) { this.attempt = attempt }
            override suspend fun clearEnrollmentAttempt(targetBinding: String?) { attempt = null }
            override suspend fun load() = saved
            override suspend fun csr() = "CSR-${++csrs}"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) { saved = trust; attempt = null }
            override suspend fun clear() { saved = null }
            override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
        }
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                seenCsrs += body!!.getValue("csr").jsonPrimitive.content
                if (seenCsrs.size == 1) { entered.complete(Unit); reply.await(); throw java.io.IOException("lost reply") }
                return WireJson.parseToJsonElement("""{"device_id":"self","certificate":"leaf","ca_certificate":"root","session":{"token":"fixture","expires_at":"2027-04-01T00:00:00Z"},"endpoints":["https://lan"]}""").jsonObject
            }
        }
        val holder = AppTrustHolder({ constructions++; keys }) { keyStore ->
            TrustController(keyStore, object : TrustTransportFactory {
                override fun enrollment(uri: EnrollUri) = api
                override suspend fun authenticated(trust: DeviceTrust) = api
            }, "android", { Instant.parse("2026-10-05T00:00:00Z") }, true)
        }
        val uri = "porchlight://enroll?host=lan&token=fixture&ca=sha256:${"ab".repeat(32)}"
        val activityScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val first = holder.controller
        val activity = activityScope.launch { first.runAction { first.enroll(uri, "fixture") }!!.join() }
        withTimeout(5000) { entered.await() }
        activityScope.cancel() // rotation destroys the UI consumer, not the enrollment action
        activity.join()
        assertSame(first, holder.controller)
        assertSame(keys, holder.keys)
        assertEquals(1, constructions)
        assertTrue(first.busy.value)
        reply.complete(Unit)
        withTimeout(5000) { first.busy.first { !it } }
        holder.controller.runAction { holder.controller.enroll(uri, "fixture") }!!.join()
        assertEquals(listOf("CSR-1", "CSR-1"), seenCsrs)
        assertEquals(1, csrs)
        assertNotNull(saved)
        assertEquals(TrustState.PinRequired, first.state.value)
        first.close()
    }

    @Test fun recreatedGateDoesNotRestoreOrRefreshTheSameSessionAgain() = runBlocking {
        var loads = 0
        val requests = mutableListOf<String>()
        var saved = DeviceTrust("d", "leaf", "root", "fixture", "2027-04-01T00:00:00Z", listOf("https://lan"), "2027-10-01T00:00:00Z")
        val keys = object : DeviceKeyStore {
            override suspend fun load(): DeviceTrust { loads++; return saved }
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) { saved = trust }
            override suspend fun clear() {}
            override fun certificateExpiry(pem: String) = saved.certificateExpiresAt
        }
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                requests += path
                return buildJsonObject {
                    if (path == "/v1/me") put("member_id", "member")
                    else put("endpoints", JsonArray(listOf(JsonPrimitive("https://lan"))))
                }
            }
        }
        val controller = TrustController(keys, object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = api
            override suspend fun authenticated(trust: DeviceTrust) = api
        }, "android", { Instant.parse("2026-10-05T00:00:00Z") }, true)
        controller.ensureRestored()
        withTimeout(5000) { controller.busy.first { !it } }
        controller.unlockPin()
        controller.refreshWhenReady()
        withTimeout(5000) { controller.busy.first { !it } }
        controller.lockPin()
        controller.ensureRestored()
        controller.unlockPin()
        controller.refreshWhenReady()
        assertEquals(1, loads)
        assertEquals(listOf("/v1/me", "/v1/endpoints"), requests)
        assertEquals(TrustState.Ready, controller.state.value)
        controller.close()
    }

    @Test fun closeDuringConnectClosesTheLateTransport() = runBlocking {
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        val closed = CompletableDeferred<Unit>()
        val keys = object : DeviceKeyStore {
            override suspend fun load() = DeviceTrust("d", "leaf", "root", null, null, listOf("https://lan"), "2027-10-01T00:00:00Z")
            override suspend fun csr() = "CSR"
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) {}
            override suspend fun clear() {}
            override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
        }
        val controller = TrustController(keys, object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri): ApiTransport = error("unused")
            override suspend fun authenticated(trust: DeviceTrust): ApiTransport {
                entered.complete(Unit); release.await()
                return object : ApiTransport {
                    override fun close() { closed.complete(Unit) }
                    override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {}
                }
            }
        }, "android", { Instant.parse("2026-10-05T00:00:00Z") }, true)
        val restoring = launch { controller.restore() }
        entered.await()
        controller.close()
        release.complete(Unit)
        withTimeout(5000) { closed.await() }
        restoring.join()
    }
}
