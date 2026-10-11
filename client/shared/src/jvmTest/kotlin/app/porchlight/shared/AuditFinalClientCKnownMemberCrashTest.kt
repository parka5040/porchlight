package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Duration.Companion.minutes

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientCKnownMemberCrashTest {
    private class Keys(val file: Path) : DeviceKeyStore {
        var afterSave: suspend () -> Unit = {}
        override suspend fun load() = WireJson.decodeFromString<DeviceTrust>(Files.readString(file))
        override suspend fun save(trust: DeviceTrust, expectedPin: String?) { Files.writeString(file, WireJson.encodeToString(trust)); afterSave() }
        override suspend fun clear() = Unit
        override suspend fun csr() = "csr"
        override fun certificateExpiry(pem: String) = "2028-10-06T12:00:00Z"
    }
    private class Server(var member: String) : TrustTransportFactory, ApiTransport {
        override fun enrollment(uri: EnrollUri) = this
        override suspend fun authenticated(trust: DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = when (path) {
            "/v1/me" -> buildJsonObject { put("member_id", member) }
            "/v1/endpoints" -> buildJsonObject { put("endpoints", JsonArray(emptyList())) }
            else -> buildJsonObject {}
        }
    }
    @Test fun processDiesInsideMeSaveBeforeCleanup() = crash(false)
    @Test fun processDiesAfterMeCacheClearBeforeNativeRetirement() = crash(true)
    private fun crash(afterCache: Boolean) = runTest {
        val fixture = AuditFinalClientCFixture(); val now = fixture.now
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "me-crash-")
        var keys = Keys(dir.resolve("fixture-trust.json")); keys.save(checkNotNull(fixture.saved))
        var server = Server("first")
        var lifetime = CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler))
        var controller = TrustController(keys, server, "desktop", { now }, false, StandardTestDispatcher(testScheduler))
        fun cache() = desktopReadCache(dir.resolve("cache.db").toFile()) { now }
        var cache = cache(); var data = HouseholdData(cache, GeneratedCall.trusted(controller), { now })
        val bus = FakeSessionBus(); var connected = true
        fun runtime() = DesktopLive(data, dir, lifetime, { now }, { _, _ -> }, {}, MutableStateFlow(null), controller,
            sessionFactory = { check(connected); bus }, sleepBusFactory = { NoSleepBus })
        var live = runtime(); var beforeNative: suspend () -> Unit = {}
        fun bind() = data.bindTrust(controller, lifetime, onIdentityEnded = { beforeNative(); live.clearIdentity(it) },
            onIdentityStarting = { owner, epoch -> live.prepareIdentity(owner, epoch) },
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        suspend fun stop() { live.close(); lifetime.cancel(); controller.close(); runCurrent(); cache.close() }
        fun reopen() {
            keys = Keys(dir.resolve("fixture-trust.json")); server = Server("first")
            lifetime = CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler))
            controller = TrustController(keys, server, "desktop", { now }, false, StandardTestDispatcher(testScheduler))
            cache = cache(); data = HouseholdData(cache, GeneratedCall.trusted(controller), { now }); live = runtime(); bind()
        }
        bind()
        val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("item")), occurrences = listOf(occurrenceRead("occ", "item")))
        try {
            controller.restore(); cache.install(snapshot); runCurrent(); stop()
            val store = DesktopAlarmStore(dir); val ledger = store.load()
            ledger.schedule("old-reminder", now, "item", "occ"); ledger.snooze("old-reminder", now + 1.minutes); store.save(ledger)
            val native = WireJson.parseToJsonElement(Files.readString(dir.resolve("os-delivery.json"))).jsonObject.toMutableMap()
            native["records"] = WireJson.encodeToJsonElement(listOf(DeliveryRecord("private", 1, handle = 7, posted = true),
                DeliveryRecord("alarm:99", 1, localAlarm = true, itemId = "item", occurrenceId = "occ")))
            Files.writeString(dir.resolve("os-delivery.json"), JsonObject(native).toString())
            connected = false; reopen(); controller.restore(); runCurrent()
            val boundary = CompletableDeferred<Unit>()
            if (afterCache) beforeNative = { boundary.complete(Unit); awaitCancellation() }
            else keys.afterSave = { if (keys.load().memberId == "second") { boundary.complete(Unit); awaitCancellation() } }
            server.member = "second"
            val replacing = lifetime.launch { controller.refreshEndpointsAndRenew() }
            boundary.await(); runCurrent()
            assertEquals("second", keys.load().memberId, "Real /me replacement was durably saved before the crash")
            if (afterCache) assertNull(cache.sync.value)
            assertEquals("first", WireJson.parseToJsonElement(Files.readString(dir.resolve("os-delivery.json"))).jsonObject
                .getValue("identity").jsonObject.getValue("memberId").jsonPrimitive.content)
            stop(); replacing.join()
            beforeNative = {}; connected = true; reopen(); server.member = "second"
            controller.restore(); runCurrent()
            assertEquals(TrustState.Ready, controller.state.value)
            assertTrue(7L in bus.closed, "Cold restore must withdraw the old private handle before a replacement snapshot")
            assertTrue(DesktopAlarmStore(dir).load().all().isEmpty())
            cache.install(snapshot.copy(cursor = 2)); runCurrent()
            assertEquals(0, bus.posts.value, "Visibility alone cannot readmit the old member's local reminder")
        } finally { stop(); dir.toFile().deleteRecursively() }
    }
}
