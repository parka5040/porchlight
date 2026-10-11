package app.porchlight.shared

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalCoroutinesApi::class)
class AuditClientDataFix1LiveRecoveryTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun root() = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-live-recovery-")
    private fun snapshot() = emptySnapshot(1).copy(items = listOf(itemRead("i")), occurrences = listOf(occurrenceRead("o", "i")),
        notifications = listOf(notificationFixture().copy(itemId = "i")),
        scheduledReminders = listOf(ScheduledReminderRead("r", "h", "self", "o", "i", "offset", now, now)))
    private fun damage(file: java.io.File) {
        val driver = openDesktopCache(file)
        try { PorchlightDb(driver).cacheQueries.putItem("i", "i", null, "{broken") } finally { driver.close() }
    }
    @Test fun rebuildingCacheDoesNotCancelSnoozesOrForgetDismissalAndActionKeys() = runTest {
        val dir = root(); val file = dir.resolve("cache.db").toFile()
        val cache = desktopReadCache(file) { now }
        val data = HouseholdData(cache, GeneratedCall({ error("Offline") }, { null }), { now })
        val bus = FakeSessionBus()
        var runtime: DesktopLive? = null
        fun delivery() = WireJson.parseToJsonElement(Files.readString(dir.resolve("os-delivery.json"))).jsonObject["records"]!!.jsonArray
        try {
            cache.install(snapshot())
            runtime = DesktopLive(data, dir, backgroundScope, { now }, { _, _ -> }, {}, MutableStateFlow(null),
                sessionFactory = { bus }, sleepBusFactory = { error("No sleep bus") })
            data.localSnooze!!("unused"); runCurrent()
            data.firedOccurrences.first { "o" in it }; runCurrent()
            val record = delivery().single { it.jsonObject["key"]?.jsonPrimitive?.content == notificationFixture().id }.jsonObject
            bus.listener!!.closed(bus.owner!!, record["handle"]!!.jsonPrimitive.long, 2)
            runCurrent()
            data.localSnooze!!("o"); runCurrent()
            val before = DesktopAlarmStore(dir).load().encode()
            val deliveries = delivery()
            assertTrue(DesktopAlarmStore(dir).load().all().single().snoozed)
            assertTrue(deliveries.single().jsonObject["dismissed"]!!.jsonPrimitive.boolean)
            damage(file); cache.initialize(); runCurrent()
            assertNull(cache.sync.value)
            assertEquals(before, DesktopAlarmStore(dir).load().encode(), "rebuilding must retain the live snooze and high-water")
            assertEquals(deliveries, delivery(), "rebuilding must retain dismissal and original action key")
            advanceTimeBy(60_001); runCurrent()
            assertEquals(before, DesktopAlarmStore(dir).load().encode(), "timer must not consume unavailable-cache tickets")
            cache.install(snapshot().copy(cursor = 2)); runCurrent()
            assertEquals(deliveries, delivery(), "recovered snapshot must not repost dismissed notifications")
            cache.install(emptySnapshot(3)); runCurrent()
            assertTrue(DesktopAlarmStore(dir).load().active().isEmpty(), "authoritative empty household still cancels")
        } finally { runtime?.close(); cache.close(); dir.toFile().deleteRecursively() }
    }

    @Test fun cacheLossResnapshotsEvenWhileExistingStreamHasHealthyHeartbeats() = runBlocking {
        val dir = root(); val file = dir.resolve("cache.db").toFile()
        val cache = desktopReadCache(file) { now }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        val requested = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val heartbeat = CompletableDeferred<Unit>()
        val transport = object : ApiTransport, TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = this
            override suspend fun authenticated(trust: DeviceTrust) = this
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = when (path.substringBefore('?')) {
                "/v1/snapshot" -> { requested.complete(Unit); release.await(); WireJson.encodeToJsonElement(snapshot().copy(cursor = 2)).jsonObject }
                "/v1/board", "/v1/agenda" -> buildJsonObject { put("items", JsonArray(emptyList())) }
                else -> buildJsonObject {}
            }
            override suspend fun stream(after: Long, session: String?, receive: suspend (StreamFrame) -> Unit) {
                while (currentCoroutineContext().isActive) { receive(StreamFrame.Heartbeat); heartbeat.complete(Unit); delay(25) }
            }
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
        try {
            cache.install(snapshot())
            data.bindTrust(controller, scope, resumeFromCursor = true)
            controller.restore()
            withTimeout(5_000) { heartbeat.await() }
            assertTrue(data.live!!.hasRecentStreamFrame())
            damage(file); cache.initialize()
            assertNotNull(withTimeoutOrNull(2_000) { requested.await(); true }, "cache loss must request a full snapshot despite healthy heartbeats")
            assertNull(cache.sync.value, "offline resnapshot remains unavailable, not authoritative empty")
            release.complete(Unit)
            withTimeout(5_000) { cache.sync.first { it?.cursor == 2L } }
            assertEquals("i", cache.view.value.items.single().id)
        } finally { release.complete(Unit); scope.cancel(); controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
