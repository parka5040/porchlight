package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class AuditClientDataChecklistTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun liveChecksAndResnapshotRefreshRetainedScopeBeforeCursor() = runTest {
        val scoped = listFixture("occurrence")
        val template = scoped.copy(occurrenceId = null)
        var checked = true
        var scopedGets = 0
        var frame: StreamFrame = StreamFrame.Tickle(2, "list_entry.changed", "list_entry", scoped.propertyEntries.first().id, false)
        val cache = memoryCache { now }
        val snapshot = emptySnapshot(1).copy(items = listOf(itemRead(collabId("item"))),
            occurrences = listOf(occurrenceRead(collabId("occurrence"), collabId("item"))), lists = listOf(template))
        val transport = object : ApiTransport, TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = this
            override suspend fun authenticated(trust: DeviceTrust) = this
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
                "/v1/snapshot" -> WireJson.encodeToJsonElement(snapshot.copy(cursor = 4)).jsonObject
                "/v1/board", "/v1/agenda" -> buildJsonObject { put("items", JsonArray(emptyList())) }
                "/v1/lists/${scoped.id}" -> WireJson.encodeToJsonElement(if ("?" in path) scoped.copy(propertyEntries = scoped.propertyEntries.map {
                    scopedGets++
                    it.copy(occurrenceId = scoped.occurrenceId, checkedAt = if (checked) now else null, checkedBy = if (checked) "self" else null)
                }) else template).jsonObject
                else -> buildJsonObject {}
            }
            override suspend fun stream(after: Long, session: String?, receive: suspend (StreamFrame) -> Unit) { receive(frame) }
        }
        val keys = object : DeviceKeyStore {
            override suspend fun load() = DeviceTrust("self", "leaf", "root", "session", "2027-01-01T00:00:00Z", listOf("https://example.invalid"), "2027-10-01T00:00:00Z")
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) = Unit
            override suspend fun clear() = Unit
            override suspend fun csr() = "CSR"
            override fun certificateExpiry(pem: String) = "2027-10-01T00:00:00Z"
        }
        val controller = TrustController(keys, transport, "desktop", { now }, false)
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        // Use bindTrust's actual dispatcher without starting its independent reconnect loop.
        val scope = CoroutineScope(Job().apply { cancel() })
        data.bindTrust(controller, scope)
        try {
            controller.restore()
            cache.install(snapshot); cache.putList(scoped)
            withBackgroundSession(controller) {
                data.live!!.connectOnce()
                assertNotNull(cache.view.value.lists.single { it.occurrenceId != null }.propertyEntries.first().checkedAt, "live check before ack (scoped reads=$scopedGets, cursor=${cache.sync.value?.cursor})")
                assertEquals(2, cache.sync.value?.cursor)
                checked = false
                frame = StreamFrame.Tickle(3, "list.changed", "list", scoped.id, false)
                data.live!!.connectOnce()
                assertNull(cache.view.value.lists.single { it.occurrenceId != null }.propertyEntries.first().checkedAt)
                assertEquals(3, cache.sync.value?.cursor)
                checked = true; frame = StreamFrame.Resnapshot
                data.live!!.connectOnce()
                assertNotNull(cache.view.value.lists.single { it.occurrenceId != null }.propertyEntries.first().checkedAt, "resnapshot check")
                assertEquals(4, cache.sync.value?.cursor)
            }
        } finally { cache.close() }
    }
}
