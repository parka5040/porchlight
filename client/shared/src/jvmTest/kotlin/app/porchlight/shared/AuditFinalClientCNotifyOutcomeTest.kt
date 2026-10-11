package app.porchlight.shared

import app.porchlight.shared.api.models.ScheduledReminderRead
import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import app.porchlight.shared.transport.WireJson
import app.porchlight.shared.trust.TrustController
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.*
import java.nio.file.*
import java.util.concurrent.CompletableFuture
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*

internal class AuditFinalClientCNotifyBus(private val delegate: FakeSessionBus = FakeSessionBus()) : DesktopSessionBus by delegate, AsyncDesktopSessionBus {
    val attempts = AtomicInteger()
    val first = CompletableDeferred<Unit>()
    val reply = CompletableFuture<Long>()
    val posts get() = delegate.posts.value
    val withdrawn = java.util.concurrent.CopyOnWriteArrayList<Long>()
    @Volatile var reject = false
    @Volatile var hold = false
    @Volatile var holdAll = false
    @Volatile var holdNumber = 1
    @Volatile var failBeforeSend = false
    @Volatile var busClosed = false
    override fun notifyAsync(owner: String, replaces: Long, title: String, body: String, actions: List<String>): CompletableFuture<Long> {
        val number = attempts.incrementAndGet()
        first.complete(Unit)
        if (failBeforeSend) throw IllegalStateException("Submission failed")
        if (reject) return CompletableFuture.failedFuture(DesktopNotifyRejected(org.freedesktop.dbus.errors.AccessDenied("Service rejected delivery")))
        val handle = delegate.notify(owner, replaces, title, body, actions)
        return if (holdAll || (hold && number == holdNumber)) reply else CompletableFuture.completedFuture(handle)
    }
    var beforeDismiss: () -> Unit = {}
    override fun dismiss(owner: String, id: Long) { beforeDismiss(); delegate.dismiss(owner, id); withdrawn += id }
    override fun close() { busClosed = true }
}

internal class AuditFinalClientCNotifyRuntime(val bus: DesktopSessionBus,
    private val deliveryWriter: (Path, String) -> Unit = ::writeDesktopDelivery,
    private val sessionFactory: () -> DesktopSessionBus = { bus },
    private val monotonicNanos: () -> Long = System::nanoTime,
    private val notifyRetryDelay: suspend (Long) -> Unit = { delay(it) }) : AutoCloseable {
    val fixture = AuditFinalClientCFixture()
    val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "notify-outcome-")
    lateinit var controller: TrustController
    lateinit var cache: ReadCache
    lateinit var data: HouseholdData
    lateinit var live: DesktopLive
    lateinit var lifetime: CoroutineScope
    val notice = MutableStateFlow<String?>(null)
    suspend fun start() {
        controller = fixture.controller(Dispatchers.Default)
        cache = desktopReadCache(dir.resolve("cache.db").toFile()) { fixture.now }
        data = HouseholdData(cache, GeneratedCall.trusted(controller), { fixture.now })
        lifetime = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        live = DesktopLive(data, dir, lifetime, { fixture.now }, { _, _ -> }, {}, notice, controller,
            sessionFactory = sessionFactory, sleepBusFactory = { NoSleepBus }, deliveryWriter = deliveryWriter,
            monotonicNanos = monotonicNanos, notifyRetryDelay = notifyRetryDelay)
        data.bindTrust(controller, lifetime, onIdentityEnded = { live.clearIdentity(it) },
            onIdentityStarting = { owner, epoch -> live.prepareIdentity(owner, epoch) },
            streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
        controller.restore()
        withTimeout(5_000) { controller.liveAdmission.first { it != null } }
    }
    suspend fun install(local: Boolean = false) {
        val snapshot = emptySnapshot(1)
        cache.install(if (!local) snapshot.copy(notifications = listOf(notificationFixture())) else snapshot.copy(
            items = listOf(itemRead("item")), occurrences = listOf(occurrenceRead("occ", "item")),
            scheduledReminders = listOf(ScheduledReminderRead("reminder", "household", snapshot.memberSettings.memberId,
                "occ", "item", "offset", fixture.now, fixture.now))))
    }
    fun records(): List<DeliveryRecord> = WireJson.decodeFromJsonElement(
        WireJson.parseToJsonElement(Files.readString(dir.resolve("os-delivery.json"))).jsonObject["records"] ?: JsonArray(emptyList()))
    suspend fun eventually(message: String, condition: () -> Boolean) {
        assertEquals(true, withTimeoutOrNull(4_000) { while (!condition()) delay(10); true }, message)
    }
    fun stop() { live.close(); lifetime.cancel(); controller.close(); cache.close() }
    override fun close() { stop(); dir.toFile().deleteRecursively() }
}

class AuditFinalClientCNotifyOutcomeTest {
    @Test fun definiteErrorRetriesServerAlertOnSameOwnerWithBackoff() = definite(false, false)
    @Test fun definiteErrorRetriesFiredReminderOnSameOwnerWithBackoff() = definite(true, false)
    @Test fun definiteErrorDoesNotPoisonServerAlertAfterRestart() = definite(false, true)
    @Test fun definiteErrorDoesNotPoisonFiredReminderAfterRestart() = definite(true, true)
    private fun definite(local: Boolean, restart: Boolean) = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { reject = true }
        AuditFinalClientCNotifyRuntime(bus).use { f ->
            f.start(); f.install(local)
            withTimeout(5_000) { bus.first.await() }
            f.eventually("Definite rejection must durably restore the non-in-flight record") { f.records().singleOrNull()?.inFlight == false }
            if (local) assertTrue(DesktopAlarmStore(f.dir).load().all().single().delivered, "Fixture must exercise an already fired reminder")
            assertTrue(f.notice.value.orEmpty().contains("notifications unavailable"))
            repeat(3) { f.cache.replaceNotifications(if (local) emptyList() else listOf(notificationFixture())); delay(40) }
            assertEquals(1, bus.attempts.get(), "Cache passes must respect the retry backoff")
            if (restart) { f.stop(); bus.reject = false; f.start() } else bus.reject = false
            f.eventually("Healthy same-owner pass must retry rejected delivery") { f.records().singleOrNull()?.posted == true }
            assertEquals(1, bus.posts, "Rejection has no native effect; later success posts exactly once")
            assertEquals(2, bus.attempts.get())
            assertFalse(f.records().single().inFlight)
        }
    }
    @Test fun submissionExceptionIsDefiniteAndRetryable() = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { failBeforeSend = true }
        AuditFinalClientCNotifyRuntime(bus).use { f ->
            f.start(); f.install(); withTimeout(5_000) { bus.first.await() }
            f.eventually("Unsent failure must clear its in-flight bit") { f.records().singleOrNull()?.inFlight == false }
            assertEquals(0, bus.posts)
            bus.failBeforeSend = false
            f.eventually("Unsent failure must retry") { f.records().singleOrNull()?.posted == true }
            assertEquals(1, bus.posts)
        }
    }
    @Test fun connectionLossAfterSendRemainsAmbiguous() = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { hold = true }
        AuditFinalClientCNotifyRuntime(bus).use { f ->
            f.start(); f.install(); withTimeout(5_000) { bus.first.await() }
            bus.reply.completeExceptionally(java.io.IOException("Connection lost after submission"))
            delay(1_200)
            repeat(3) { f.cache.replaceNotifications(listOf(notificationFixture())); delay(40) }
            assertTrue(f.records().single().inFlight)
            assertEquals(1, bus.attempts.get(), "Transport failure is not proof of no native effect")
        }
    }
    @Test fun newerVersionDoesNotReuseTheHandleOfAnAmbiguousReplacement() = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { hold = true; holdNumber = 2 }
        AuditFinalClientCNotifyRuntime(bus).use { f ->
            try {
                f.start(); f.install()
                f.eventually("Original handle must be durable") { f.records().singleOrNull()?.handle == 1L }
                val first = notificationFixture()
                f.cache.replaceNotifications(listOf(first.copy(version = first.version + 1)))
                f.eventually("Replacement must be pending") { bus.attempts.get() == 2 && f.records().single().inFlight }
                f.cache.replaceNotifications(listOf(first.copy(version = first.version + 2)))
                f.eventually("Newer version must use a separate handle from the ambiguous replacement") { f.records().singleOrNull()?.handle == 2L }
                bus.reply.complete(1L)
                f.eventually("Late replaced handle must be withdrawn") { 1L in bus.withdrawn }
                assertFalse(2L in bus.withdrawn, "Late old completion must not withdraw current content")
                assertEquals(2L, f.records().single().handle)
            } finally { bus.reply.complete(1L) }
        }
    }
    @Test fun newerVersionSupersedesAmbiguousCallAndWithdrawsLateOldHandle() = newer(false)
    @Test fun newerVersionSupersedesUnresolvedSameDaemonRecordAfterRestart() = newer(true)
    private fun newer(restart: Boolean) = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { hold = true }
        AuditFinalClientCNotifyRuntime(bus).use { f ->
            try {
                f.start(); f.install()
                withTimeout(5_000) { bus.first.await() }; delay(2_100)
                assertEquals(1, bus.attempts.get())
                assertTrue(f.records().single().inFlight)
                if (restart) { f.stop(); f.start(); delay(100); assertEquals(1, bus.attempts.get()) }
                val old = notificationFixture()
                f.cache.replaceNotifications(listOf(old.copy(version = old.version + 1, summary = old.summary.copy(title = "Updated alert"))))
                f.eventually("Newer server content must post even when the earlier version is ambiguous") { bus.attempts.get() == 2 }
                f.eventually("New version handle must be durable") { f.records().singleOrNull()?.handle == 2L }
                if (!restart) {
                    bus.reply.complete(1L)
                    f.eventually("Late old-version handle must be withdrawn") { 1L in bus.withdrawn }
                    assertEquals(2L, f.records().single().handle)
                }
                assertEquals(2, bus.posts)
            } finally { bus.reply.complete(1L) }
        }
    }
}
