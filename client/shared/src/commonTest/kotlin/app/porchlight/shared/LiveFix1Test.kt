package app.porchlight.shared

import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.minutes

@OptIn(ExperimentalCoroutinesApi::class)
class LiveFix1Test {
    private val epoch = Instant.fromEpochMilliseconds(0)
    @Test fun snoozeOnlyDueTicketsAndAuthoritativeFutureTimeRearms() {
        val ledger = AlarmLedger()
        val due = ledger.schedule("due", epoch, "i", "o")
        val future = ledger.schedule("future", epoch + 1.days, "i", "o")
        assertEquals(listOf(due), ledger.snoozable("o", epoch))
        ledger.snooze(due.key, epoch + 15.minutes)
        assertEquals(future, ledger.schedule("future", future.fireAt, "i", "o"))
        val snoozed = ledger.ticket(due.code)!!
        ledger.fired(snoozed.code, snoozed.generation)
        assertFalse(ledger.ticket(due.code)!!.snoozed)
        val rearmed = ledger.schedule("due", epoch + 2.days, "i", "o")
        assertFalse(rearmed.delivered)
        assertEquals(epoch + 2.days, rearmed.fireAt)
    }
    @Test fun serverRescheduleOverridesPendingSnoozeButUnchangedSourceDoesNot() {
        val ledger = AlarmLedger()
        val original = ledger.schedule("r", epoch, "i", "o")
        ledger.snooze("r", epoch + 15.minutes)
        assertTrue(ledger.schedule("r", epoch, "i", "o").snoozed)
        val changed = ledger.schedule("r", epoch + 1.days, "i", "o")
        assertFalse(changed.snoozed)
        assertTrue(changed.generation > original.generation)
    }
    @Test fun pruningPreservesCodesAndStaleIntentFence() {
        val ledger = AlarmLedger()
        val old = ledger.schedule("old", epoch, "i", "o")
        ledger.cancel(old.key)
        ledger.prune(epoch + 8.days)
        assertNull(ledger.ticket(old.code))
        val restored = AlarmLedger.decode(ledger.encode())
        val newer = restored.schedule("old", epoch + 9.days, "i", "o")
        assertTrue(newer.code > old.code)
        assertFalse(restored.canFire(old.code, old.generation))
    }
    @Test fun originalAndroidDeliveryKeySurvivesWriteAheadRetryAndGateDismissal() {
        val inventory = DeliveryInventory()
        val pending = inventory.prepareServer("n", 2, "original")
        assertEquals("original", pending.actionKey)
        val restarted = DeliveryInventory.decode(inventory.encode())
        assertEquals("original", restarted.prepareServer("n", 2, "replacement").actionKey)
        restarted.posted("n")
        val delivered = DeliveryInventory.decode(restarted.encode()).requirePostedAction("n")
        assertEquals("original", delivered.actionKey)
        restarted.dismissed("n")
        assertFails { restarted.requirePostedAction("n") }
    }
    @Test fun controllerDismissalPathUsesPersistedKeyOnActualRepositoryRequest() = runTest {
        val n = notificationFixture()
        val cache = app.porchlight.shared.data.memoryCache { epoch }
        val seen = mutableListOf<Pair<String, String?>>()
        val transport = object : app.porchlight.shared.transport.ApiTransport {
            override suspend fun request(method: String, path: String, body: kotlinx.serialization.json.JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): kotlinx.serialization.json.JsonObject {
                assertEquals("POST", method); assertEquals("{}", body.toString())
                seen += path to idempotencyKey
                return app.porchlight.shared.transport.WireJson.encodeToJsonElement(app.porchlight.shared.api.models.Notification.serializer(), n.copy(readAt = epoch)) as kotlinx.serialization.json.JsonObject
            }
        }
        val data = app.porchlight.shared.data.HouseholdData(cache, app.porchlight.shared.data.GeneratedCall({ transport }, { "session" }), { epoch })
        data.notifications.afterWrite = {} // this test server exposes only the action route
        try {
            cache.install(app.porchlight.shared.data.emptySnapshot(1).copy(notifications = listOf(n)))
            val inventory = DeliveryInventory()
            inventory.prepareServer(n.id, n.version.toLong(), "original"); inventory.posted(n.id)
            val restarted = DeliveryInventory.decode(inventory.encode())
            assertIs<app.porchlight.shared.data.WriteResult.Success<*>>(dismissDeliveredNotification(data, restarted, n.id))
            assertEquals(listOf<Pair<String, String?>>("/v1/notifications/${n.id}/read" to "original"), seen)
        } finally { data.close() }
    }
    @Test fun readyAloneNeverResetsBackoffAndNetworkWakesIt() = runTest {
        val starts = mutableListOf<Long>()
        val live = LiveSync(StreamSource { _, receive -> starts += testScheduler.currentTime; receive(StreamFrame.Ready); throw InvalidStream() },
            { 1 }, {}, {}, {}, { Instant.fromEpochMilliseconds(testScheduler.currentTime) }, { 0.0 })
        val job = backgroundScope.launch { live.run() }
        runCurrent(); advanceTimeBy(3001); runCurrent()
        assertEquals(listOf(0L, 1000L, 3000L), starts)
        live.networkChanged(); runCurrent()
        assertEquals(3001L, starts.last())
        job.cancel(); runCurrent(); assertEquals(ConnectionState.Stopped, live.state.value)
    }
    @Test fun heartbeatTimeoutReconnectsAndRealHeartbeatResetsBackoff() = runTest {
        val starts = mutableListOf<Long>()
        val live = LiveSync(StreamSource { _, receive ->
            starts += testScheduler.currentTime
            if (starts.size == 1) throw InvalidStream()
            if (starts.size == 2) { receive(StreamFrame.Heartbeat); throw InvalidStream() }
            awaitCancellation()
        }, { 1 }, {}, {}, {}, { Instant.fromEpochMilliseconds(testScheduler.currentTime) }, { 0.0 })
        backgroundScope.launch { live.run() }
        runCurrent(); advanceTimeBy(2001); runCurrent()
        assertEquals(listOf(0L, 1000L, 2000L), starts)
        advanceTimeBy(252_001); runCurrent()
        assertEquals(254000L, starts.last()) // two 120s heartbeats + 10s margin, then 2s backoff
    }
    @Test fun quietStreamSurvivesMoreThanThreeProductionHeartbeatIntervals() = runTest {
        var connects = 0
        val live = LiveSync(StreamSource { _, receive ->
            connects++; receive(StreamFrame.Ready)
            while (true) { delay(120_000); receive(StreamFrame.Heartbeat) }
        }, { 1 }, {}, {}, {}, { Instant.fromEpochMilliseconds(testScheduler.currentTime) }, { 0.0 })
        backgroundScope.launch { live.run() }
        runCurrent(); advanceTimeBy(480_001); runCurrent()
        assertEquals(1, connects); assertEquals(ConnectionState.Connected, live.state.value)
    }
    @Test fun optedInSafetyNetAwaitsUnreadReadWithoutHealthyStreamSnapshot() = runTest {
        var unread = 0; var refresh = 0
        liveSafetyNet(true, true, { delay(10); unread++ }, { refresh++ })
        assertEquals(1, unread); assertEquals(0, refresh)
        liveSafetyNet(true, false, { unread++ }, { delay(10); refresh++ })
        assertEquals(1, refresh)
    }
    @Test fun staleNetworkWakeDoesNotSkipBackoffAndNetworkCancelsActiveStream() = runTest {
        var connects = 0
        val live = LiveSync(StreamSource { _, _ -> if (++connects == 1) throw InvalidStream() else awaitCancellation() },
            { 1 }, {}, {}, {}, { Instant.fromEpochMilliseconds(testScheduler.currentTime) }, { 0.0 })
        live.networkChanged() // default NetworkCallback's initial notification, before connecting
        backgroundScope.launch { live.run() }; runCurrent()
        assertEquals(1, connects)
        advanceTimeBy(1001); runCurrent(); assertEquals(2, connects)
        live.networkChanged(); runCurrent(); assertEquals(3, connects)
        advanceTimeBy(2001); runCurrent(); assertEquals(3, connects)
    }
    @Test fun reachabilityRearmsWhenWallClockLagsAtDeadline() = runTest {
        var wall = epoch
        val live = LiveSync(StreamSource { _, _ -> awaitCancellation() }, { 1 }, {}, {}, {}, { wall })
        backgroundScope.launch { live.run() }; runCurrent()
        advanceTimeBy(300_001); runCurrent(); assertNull(live.unreachableSince.value)
        wall = epoch + 6.minutes
        advanceTimeBy(60_001); runCurrent(); assertNotNull(live.unreachableSince.value)
    }
    @Test fun actionOwnerSurvivesSessionRotationButNotIdentityEnd() {
        assertEquals("original", actionIdentityOwner("device:member", "device:member", "original") { "new" })
        assertEquals("new", actionIdentityOwner("device:member", null, null) { "new" })
        assertEquals("new", actionIdentityOwner("device:other", "device:member", "original") { "new" })
        assertEquals("Porchlight: household connection", notificationPublicTitle("connection"))
        assertEquals("Porchlight: 1 reminder", notificationPublicTitle("reminders"))
    }
    @Test fun overdueArmedDesktopTicketSurvivesOnlyWhileAuthoritativeReminderStillMatches() = runTest {
        val cache = app.porchlight.shared.data.memoryCache { epoch }
        try {
            val reminder = app.porchlight.shared.api.models.ScheduledReminderRead("r", "h", "self", "o", "i", "offset", epoch + 1.days, epoch)
            cache.install(app.porchlight.shared.data.emptySnapshot(1).copy(
                items = listOf(app.porchlight.shared.data.itemRead("i")), occurrences = listOf(app.porchlight.shared.data.occurrenceRead("o", "i")),
                scheduledReminders = listOf(reminder)))
            val ledger = AlarmLedger(); val ticket = ledger.schedule("r", reminder.fireAt, "i", "o")
            val resume = epoch + 1.days + 5.minutes
            reconcileAlarms(ledger, cache.view.value, resume, preserveOverdue = true)
            assertEquals(ticket, ledger.active().single())
            reconcileAlarms(ledger, cache.view.value.copy(reminders = emptyList()), resume, preserveOverdue = true)
            assertTrue(ledger.active().isEmpty()) // removal is not overdue preservation
        } finally { cache.close() }
    }
    @Test fun drainReconnectsWithoutSnapshotAndReachabilityUsesDeadlineNotTicks() = runTest {
        var starts = 0; var snapshots = 0
        val live = LiveSync(StreamSource { _, receive -> if (++starts == 1) receive(StreamFrame.Drain) else awaitCancellation() },
            { 1 }, {}, {}, { snapshots++ }, { Instant.fromEpochMilliseconds(testScheduler.currentTime) }, { 0.0 })
        backgroundScope.launch { live.run() }
        runCurrent(); advanceTimeBy(1001); runCurrent()
        assertEquals(2, starts); assertEquals(0, snapshots)
        advanceTimeBy(300_000); runCurrent()
        assertNotNull(live.unreachableSince.value)
        live.successfulRequest(); runCurrent(); assertNull(live.unreachableSince.value)
    }
}
