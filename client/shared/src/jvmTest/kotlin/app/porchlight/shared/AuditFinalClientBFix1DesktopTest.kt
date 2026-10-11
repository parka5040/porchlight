package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import java.nio.file.*
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientBFix1DesktopTest {
    private fun directory() = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "live-fix1-")
    @Test fun coldPendingLogoutCannotDeliverWhileKeyLoadIsPaused() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        val cache = memoryCache { f.now }
        val data = HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now })
        val dir = directory(); val bus = FakeSessionBus()
        val loaded = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>(); val pass = CompletableDeferred<Unit>()
        f.saved = f.saved.withLogoutPending(true)
        f.beforeLoad = { loaded.complete(Unit); release.await() }
        cache.install(emptySnapshot(1).copy(notifications = listOf(notificationFixture())))
        val live = DesktopLive(data, dir, backgroundScope, { f.now }, { _, _ -> }, {}, MutableStateFlow(null), f.controller,
            sessionFactory = { bus }, sleepBusFactory = { NoSleepBus }, onAlarmWait = { pass.complete(Unit) })
        f.controller.identityEnded = { cache.clear(); live.clearIdentity() }
        val restore = launch { f.controller.restore() }
        try {
            loaded.await(); pass.await(); runCurrent()
            assertEquals(0, bus.posts.value, "Cold pending logout must not deliver before key loading/cleanup")
            release.complete(Unit); restore.join(); runCurrent()
            assertEquals(0, bus.posts.value)
        } finally { release.complete(Unit); live.close(); f.controller.close(); cache.close(); runCurrent(); dir.toFile().deleteRecursively() }
    }
    @Test fun failedCacheClearRetriesNativeCleanupAndReopensDelivery() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        var fail = false; var clears = 0
        val files = object : AttachmentStore by MemoryAttachmentStore() {
            override suspend fun clear() { clears++; if (fail) { fail = false; error("fail-once private cleanup") } }
        }
        val cache = memoryCache(files) { f.now }
        val data = HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now })
        val dir = directory(); val bus = FakeSessionBus()
        f.controller.restore()
        cache.install(emptySnapshot(1).copy(notifications = listOf(notificationFixture())))
        val live = DesktopLive(data, dir, backgroundScope, { f.now }, { _, _ -> }, {}, MutableStateFlow(null), f.controller,
            sessionFactory = { bus }, sleepBusFactory = { NoSleepBus })
        data.bindTrust(f.controller, backgroundScope, onIdentityEnded = { live.clearIdentity() },
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        try {
            bus.posts.first { it > 0 }; runCurrent(); assertEquals(1, bus.posts.value)
            fail = true; f.switchMember(); f.controller.liveAdmission.first { it == 1L }; runCurrent()
            assertTrue(clears >= 2)
            assertTrue(bus.closed.isNotEmpty(), "Autonomous retry must also withdraw old native delivery")
            cache.install(emptySnapshot(2).copy(notifications = listOf(notificationFixture().copy(id = "new-alert"))))
            runCurrent()
            assertEquals(2, bus.posts.value, "Replacement identity must deliver without restart")
        } finally { live.close(); f.controller.close(); cache.close(); runCurrent(); dir.toFile().deleteRecursively() }
    }
    @Test fun disconnectedCleanupRetiresPendingLocalAlarmForever() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        val cache = memoryCache { f.now }; val data = HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now })
        val dir = directory(); val bus = FakeSessionBus(); var connected = false
        val record = DeliveryRecord("alarm:1", 1, localAlarm = true, itemId = "item", occurrenceId = "occ")
        Files.writeString(dir.resolve("os-delivery.json"), buildJsonObject { put("records", WireJson.encodeToJsonElement(listOf(record))) }.toString())
        f.controller.restore()
        val live = DesktopLive(data, dir, backgroundScope, { f.now }, { _, _ -> }, {}, MutableStateFlow(null), f.controller,
            sessionFactory = { check(connected); bus }, sleepBusFactory = { NoSleepBus })
        f.controller.identityEnded = { cache.clear(); live.clearIdentity() }
        try {
            runCurrent(); f.switchMember()
            val retired = WireJson.parseToJsonElement(Files.readString(dir.resolve("os-delivery.json"))).jsonObject["records"]?.jsonArray.orEmpty()
            assertTrue(retired.isEmpty(), "Disconnected identity cleanup must retire all deliverable records")
            cache.install(emptySnapshot(2).copy(items = listOf(itemRead("item")), occurrences = listOf(occurrenceRead("occ", "item"))))
            connected = true; advanceTimeBy(15_100); runCurrent()
            assertEquals(0, bus.posts.value, "New member has no reminder: old pending local alarm is never repostable")
        } finally { live.close(); f.controller.close(); cache.close(); runCurrent(); dir.toFile().deleteRecursively() }
    }
    @Test fun disconnectedPostedHandleSurvivesOnlyAsOwnerBoundWithdrawal() = runTest {
        val f = AuditFinalClientBFix1Fixture(StandardTestDispatcher(testScheduler))
        val cache = memoryCache { f.now }; val data = HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now })
        val dir = directory(); val bus = FakeSessionBus(); var connected = false; var opened = 0
        val record = DeliveryRecord("alarm:7", 1, handle = 7, posted = true, localAlarm = true, itemId = "item", occurrenceId = "occ")
        Files.writeString(dir.resolve("os-delivery.json"), buildJsonObject {
            put("owner", bus.owner); put("records", WireJson.encodeToJsonElement(listOf(record)))
        }.toString())
        f.controller.restore()
        val live = DesktopLive(data, dir, backgroundScope, { f.now }, { _, _ -> opened++ }, {}, MutableStateFlow(null), f.controller,
            sessionFactory = { check(connected); bus }, sleepBusFactory = { NoSleepBus })
        f.controller.identityEnded = { cache.clear(); live.clearIdentity() }
        try {
            f.controller.logout()
            val retired = WireJson.parseToJsonElement(Files.readString(dir.resolve("os-delivery.json"))).jsonObject
            assertTrue(retired["records"]?.jsonArray.orEmpty().isEmpty())
            assertEquals(1, retired.getValue("withdrawals").jsonArray.size)
            cache.install(emptySnapshot(2).copy(items = listOf(itemRead("item")), occurrences = listOf(occurrenceRead("occ", "item"))))
            connected = true; advanceTimeBy(15_100); runCurrent()
            assertEquals(listOf(7L), bus.closed)
            assertEquals(0, bus.posts.value)
            bus.action(checkNotNull(bus.owner), 7, "default"); runCurrent(); assertEquals(0, opened)
        } finally { live.close(); f.controller.close(); cache.close(); runCurrent(); dir.toFile().deleteRecursively() }
    }
    @Test fun stalledBusBacklogCannotBlockEpochOrCacheCommits() = stalledBus(false)
    @Test fun timedOutBusLetsLogoutFinishWithoutDrainingTheBacklog() = stalledBus(true)
    private fun stalledBus(timeout: Boolean) = runBlocking {
        val f = AuditFinalClientBFix1Fixture(Dispatchers.Default)
        val cache = memoryCache { f.now }; val data = HouseholdData(cache, GeneratedCall.trusted(f.controller), { f.now })
        val dir = directory(); val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val bus = FakeSessionBus()
        val slow = object : DesktopSessionBus by bus {
            override fun notify(owner: String, replaces: Long, title: String, body: String, actions: List<String>): Long {
                entered.countDown()
                check(release.await(if (timeout) 750 else 5_000, TimeUnit.MILLISECONDS)) { "Native reply timeout" }
                return bus.notify(owner, replaces, title, body, actions)
            }
        }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        f.controller.restore()
        cache.sessionEpoch = { f.controller.sessionEpoch.value }; cache.sessionCommitMutex = f.controller.sessionCommitMutex
        cache.install(emptySnapshot(1).copy(notifications = (1..100).map { notificationFixture().copy(id = "alert-$it") }))
        val live = DesktopLive(data, dir, scope, { f.now }, { _, _ -> }, {}, MutableStateFlow(null), f.controller,
            sessionFactory = { slow }, sleepBusFactory = { NoSleepBus })
        f.controller.identityEnded = { cache.clear(); live.clearIdentity() }
        try {
            assertTrue(withContext(Dispatchers.IO) { entered.await(5, TimeUnit.SECONDS) })
            val logout = scope.launch { f.controller.logout() }
            val advanced = withTimeoutOrNull(500) { f.controller.sessionEpoch.first { it > 0 } }
            assertNotNull(advanced, "Stalled native work must not hold the identity/cache commit lock")
            assertNotNull(withTimeoutOrNull(500) {
                cache.view.first { it.sync == null }
                assertFailsWith<CacheOwnerUnavailable> { cache.inSession { error("Logged-out owner admitted") } }
                // This unbound runtime fixture explicitly readmits an owner before installing household rows.
                cache.initialize(f.saved.cacheOwner())
                cache.install(emptySnapshot(2)); true
            }, "Unrelated cache commit must be independent of bus backlog")
            if (!timeout) release.countDown()
            withTimeout(1_500) { logout.join() }
            val posts = bus.posts.value; delay(100)
            assertEquals(posts, bus.posts.value); assertTrue(posts <= 1, "Identity end takes priority between backlog units")
        } finally { release.countDown(); live.close(); scope.cancel(); f.controller.close(); cache.close(); dir.toFile().deleteRecursively() }
    }
}
