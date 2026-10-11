package app.porchlight.shared

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.data.*
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.trust.TrustState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.encodeToString
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Duration.Companion.minutes

/** Real binding + Android runtime; fake native buses persist across process-shaped object replacement. */
@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientCAndroidOwnerTest {
    private class Native(val dir: Path) {
        var fail = false
        var posts = 0
        var withdrawals = 0
        fun owner() = dir.resolve("owner.json").takeIf(Files::exists)?.let { WireJson.decodeFromString<CacheOwner>(Files.readString(it)) }
        fun owner(value: CacheOwner?) { if (value == null) Files.deleteIfExists(dir.resolve("owner.json")) else Files.writeString(dir.resolve("owner.json"), WireJson.encodeToString(value)) }
        fun ledger() = dir.resolve("alarms.json").takeIf(Files::exists)?.let { AlarmLedger.decode(Files.readString(it)) } ?: AlarmLedger()
        fun ledger(value: AlarmLedger) { Files.writeString(dir.resolve("alarms.json"), value.encode()) }
        fun inventory() = dir.resolve("delivery.json").takeIf(Files::exists)?.let { DeliveryInventory.decode(Files.readString(it)) } ?: DeliveryInventory()
        fun inventory(value: DeliveryInventory) { Files.writeString(dir.resolve("delivery.json"), value.encode()) }
        fun clear() {
            check(!fail) { "Native storage unavailable" }
            ledger(ledger().also { it.retire() })
            withdrawals += inventory().records().count { it.posted }
            inventory(DeliveryInventory())
        }
        fun deliver(view: CacheView) { posts += inventory().localToPost(view).size }
    }
    @Test fun restartAfterSaveRetiresOldAndroidInventories() = restart(false)
    @Test fun restartAfterSqlPurgeStillRetiresOldAndroidInventories() = restart(true)
    @Test fun failedNativePurgeDoesNotConsumeTheOnlyOwnerMarker() = restart(true, fail = true)
    @Test fun missingOwnerRetiresLegacyAndroidInventories() = restart(false, missing = true)
    private fun restart(cachePurged: Boolean, fail: Boolean = false, missing: Boolean = false) = runTest {
        val f = AuditFinalClientCFixture()
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "android-owner-")
        fun cache(): ReadCache {
            val file = dir.resolve("cache.db"); val create = !Files.exists(file)
            val driver = JdbcSqliteDriver("jdbc:sqlite:$file")
            if (create) PorchlightDb.Schema.create(driver)
            return ReadCache({ driver }, { f.now }, null, MemoryAttachmentStore())
        }
        var native = Native(dir)
        var cache = cache()
        var controller = f.controller(StandardTestDispatcher(testScheduler))
        var lifetime = CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler))
        fun runtime() = AndroidLiveReconciler(controller, { cache.view.value }, { view ->
            native.ledger(native.ledger().also { reconcileAndroidAlarms(it, view, f.now) })
        }, native::deliver, { native.posts += native.ledger().active().size }, native::clear,
            loadOwner = native::owner, saveOwner = native::owner)
        var runtime = runtime()
        fun bind() = HouseholdData(cache, GeneratedCall.trusted(controller), { f.now }).bindTrust(controller, lifetime,
            onIdentityEnded = { runtime.clearIdentity(it) }, onIdentityStarting = { owner, epoch -> runtime.prepareIdentity(owner, epoch) },
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        bind()
        val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("item")), occurrences = listOf(occurrenceRead("occ", "item")))
        try {
            controller.restore(); cache.install(snapshot)
            native.ledger(AlarmLedger().also { it.schedule("reminder", f.now, "item", "occ"); it.snooze("reminder", f.now + 1.minutes) })
            native.inventory(DeliveryInventory().also {
                it.put(DeliveryRecord("private", 1, posted = true))
                it.put(DeliveryRecord("alarm:99", 1, localAlarm = true, itemId = "item", occurrenceId = "occ"))
            })
            lifetime.cancel(); controller.close(); runCurrent(); cache.close()
            if (missing) native.owner(null)
            f.saved = checkNotNull(f.saved).withIdentity("second", emptyList())
            if (cachePurged) { cache = cache(); cache.initialize(checkNotNull(f.saved).cacheOwner()); cache.close() }
            native = Native(dir); native.fail = fail
            cache = cache(); controller = f.controller(StandardTestDispatcher(testScheduler))
            lifetime = CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler)); runtime = runtime(); bind()
            controller.restore()
            if (fail) {
                assertTrue(controller.state.value is TrustState.LocalTrustLost)
                assertNull(controller.liveAdmission.value)
                assertEquals("first", native.owner()?.memberId, "Failed native retirement must retain the mismatch marker")
                lifetime.cancel(); controller.close(); runCurrent(); cache.close()
                native = Native(dir); cache = cache(); controller = f.controller(StandardTestDispatcher(testScheduler))
                lifetime = CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler)); runtime = runtime(); bind()
                controller.restore()
            }
            assertEquals(TrustState.Ready, controller.state.value)
            assertEquals(1, native.withdrawals, "Old posted handles must be withdrawal-only on replacement startup")
            assertTrue(native.ledger().all().isEmpty())
            assertTrue(native.inventory().records().isEmpty())
            cache.install(snapshot.copy(cursor = 2)); runtime.reconcile(recover = true)
            assertEquals(0, native.posts, "Visible occurrence is not a new-member reminder")
            assertEquals("second", native.owner()?.memberId)
        } finally { lifetime.cancel(); controller.close(); runCurrent(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
