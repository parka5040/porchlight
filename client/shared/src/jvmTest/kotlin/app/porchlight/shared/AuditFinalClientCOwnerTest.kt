package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Duration.Companion.minutes

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientCOwnerTest {
    @Test fun restartAfterReplacementSaveRetiresBothNativeInventories() = restart(false)
    @Test fun restartAfterCachePurgeRetiresBothNativeInventories() = restart(true)
    @Test fun missingNativeOwnerRetiresLegacyInventories() = restart(false, missing = true)
    private fun restart(cachePurged: Boolean, missing: Boolean = false) = runTest {
        val f = AuditFinalClientCFixture()
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "native-owner-")
        val old = checkNotNull(f.saved).cacheOwner()
        val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("item")), occurrences = listOf(occurrenceRead("occ", "item")))
        fun cache() = desktopReadCache(dir.resolve("cache.db").toFile()) { f.now }
        var cache = cache()
        var controller = f.controller(StandardTestDispatcher(testScheduler))
        var data = HouseholdData(cache, GeneratedCall.trusted(controller), { f.now })
        var lifetime = CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler))
        val bus = FakeSessionBus()
        var live = DesktopLive(data, dir, lifetime, { f.now }, { _, _ -> }, {}, MutableStateFlow(null), controller,
            sessionFactory = { bus }, sleepBusFactory = { NoSleepBus })
        data.bindTrust(controller, lifetime, onIdentityEnded = { live.clearIdentity(it) },
            onIdentityStarting = { owner, epoch -> live.prepareIdentity(owner, epoch) },
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        try {
            controller.restore(); cache.install(snapshot); runCurrent()
            live.close(); lifetime.cancel(); controller.close(); runCurrent(); cache.close()
            // Crash image: a snoozed alarm, a pending local reminder, and an already posted private handle.
            val store = DesktopAlarmStore(dir); val ledger = store.load()
            val ticket = ledger.schedule("old-reminder", f.now, "item", "occ")
            ledger.snooze(ticket.key, f.now + 1.minutes); store.save(ledger)
            val posted = DeliveryRecord("old-private", 1, handle = 7, posted = true)
            val pending = DeliveryRecord("alarm:99", 1, localAlarm = true, itemId = "item", occurrenceId = "occ")
            val saved = WireJson.parseToJsonElement(Files.readString(dir.resolve("os-delivery.json"))).jsonObject.toMutableMap()
            saved["owner"] = JsonPrimitive(bus.owner)
            saved["records"] = WireJson.encodeToJsonElement(listOf(posted, pending))
            if (missing) saved.remove("identity")
            Files.writeString(dir.resolve("os-delivery.json"), JsonObject(saved).toString())
            f.saved = checkNotNull(f.saved).withIdentity("second", emptyList()) // durable replacement save, before cleanup
            if (cachePurged) { cache = cache(); cache.initialize(checkNotNull(f.saved).cacheOwner()); cache.close() }
            cache = cache(); controller = f.controller(StandardTestDispatcher(testScheduler))
            data = HouseholdData(cache, GeneratedCall.trusted(controller), { f.now })
            lifetime = CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler))
            live = DesktopLive(data, dir, lifetime, { f.now }, { _, _ -> }, {}, MutableStateFlow(null), controller,
                sessionFactory = { bus }, sleepBusFactory = { NoSleepBus })
            data.bindTrust(controller, lifetime, onIdentityEnded = { live.clearIdentity(it) },
            onIdentityStarting = { owner, epoch -> live.prepareIdentity(owner, epoch) },
                streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
            controller.restore(); runCurrent()
            assertEquals(TrustState.Ready, controller.state.value)
            assertTrue(7L in bus.closed, "Restart must withdraw old private handles before any replacement snapshot")
            assertTrue(DesktopAlarmStore(dir).load().all().isEmpty(), "Restart must retire old snoozed alarms")
            cache.install(snapshot.copy(cursor = 2)); runCurrent()
            assertEquals(0, bus.posts.value, "Visible occurrence without a replacement reminder must not repost old local delivery")
            assertNotEquals(old, checkNotNull(f.saved).cacheOwner())
        } finally { live.close(); lifetime.cancel(); controller.close(); runCurrent(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
