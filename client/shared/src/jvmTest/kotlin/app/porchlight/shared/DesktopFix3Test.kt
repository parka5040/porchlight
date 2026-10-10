package app.porchlight.shared

import app.porchlight.shared.live.*
import app.porchlight.shared.data.*
import app.porchlight.shared.settings.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

@OptIn(ExperimentalCoroutinesApi::class)
class DesktopFix3Test {
    private fun directory() = Files.createTempDirectory(Path.of(System.getProperty("porchlight.test.dir")), "fix3-")
    private class FakeSleepBus : DesktopSleepBus {
        lateinit var signal: (Boolean) -> Unit
        var closed = false
        override fun watch(receive: (Boolean) -> Unit) { signal = receive }
        override fun close() { closed = true }
    }
    @Test fun resumeSignalCancelsStaleStreamAndReconnectsWithoutBackoff() = runTest {
        var starts = 0
        val live = LiveSync(StreamSource { _, receive -> starts++; receive(StreamFrame.Ready); awaitCancellation() },
            { 1 }, {}, {}, {}, { Instant.fromEpochMilliseconds(testScheduler.currentTime) })
        val bus = FakeSleepBus()
        val monitor = DesktopResumeMonitor(live::networkChanged) { bus }
        try {
            backgroundScope.launch { live.run() }
            runCurrent(); assertEquals(1, starts)
            bus.signal(true); runCurrent(); assertEquals(1, starts)
            bus.signal(false); runCurrent(); assertEquals(2, starts)
            assertEquals(0L, testScheduler.currentTime)
        } finally { monitor.close() }
        assertTrue(bus.closed)
    }
    @Test fun failedAlarmRecoveryLeavesNoticeNoopAlarmsAndLiveInbox() = runTest {
        val dir = directory()
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall({ error("No network") }, { null }), { now })
        val notice = MutableStateFlow<String?>(null)
        val session = FakeSessionBus()
        var runtime: DesktopLive? = null
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("i")), occurrences = listOf(occurrenceRead("o", "i")),
                scheduledReminders = listOf(app.porchlight.shared.api.models.ScheduledReminderRead("r", "h", "self", "o", "i", "offset", now + 1.days, now))))
            runtime = DesktopLive(data, dir, backgroundScope, { now }, { _, _ -> }, {}, notice,
                sessionFactory = { session }, alarmLoader = { error("recovery counter write failed") }, sleepBusFactory = { FakeSleepBus() })
            data.localSnooze!!("o") // await asynchronous cache initialization/recovery
            runCurrent()
            assertTrue(notice.value.orEmpty().contains("inventory recovery failed"))
            assertNotNull(runtime.alarmWaitingUntil) // alarm loop finished initialization instead of dying
            assertNull(runtime.alarmWaitingUntil.value)
            data.localSnooze!!("o") // degraded local ledger is a safe no-op, not a failed initialization Deferred
            assertTrue(data.firedOccurrences.value.isEmpty())
            assertFalse(Files.exists(dir.resolve("alarm-tickets"))) // no unsafe reset of durable codes
            assertTrue(session.ownerCalls > 0)
            cache.replaceNotifications(listOf(notificationFixture()))
            session.posts.first { it == 1 } // Inbox/OS notification collector survives degraded alarms
            session.watcher = false; session.listener!!.watcherChanged(); runCurrent()
            assertTrue(notice.value.orEmpty().contains("inventory recovery failed"))
        } finally { runtime?.close(); data.close(); dir.toFile().deleteRecursively() }
    }
    @Test fun serviceNiriConflictPersistsOffAcrossDatastoreRestartAndKeepsMessage() = runTest {
        val dir = directory()
        val file = dir.resolve("device.preferences_pb").toFile()
        var job = SupervisorJob()
        fun store() = DataStoreSettingsStore(androidx.datastore.preferences.core.PreferenceDataStoreFactory.create(
            scope = CoroutineScope(job + Dispatchers.IO), produceFile = { file }))
        try {
            Files.createDirectories(dir.resolve("niri"))
            Files.writeString(dir.resolve("niri/config.kdl"), niriAutostart("/opt/porchlight/bin/porchlight"))
            val settings = store()
            settings.setStayConnected(true)
            val conflict = MutableStateFlow<String?>(null)
            applyDesktopLoginSetting(dir, "/opt/porchlight/bin/porchlight", true, settings, conflict)
            assertFalse(settings.settings.first().stayConnected)
            assertTrue(conflict.value.orEmpty().contains("turned off"))
            applyDesktopLoginSetting(dir, "/opt/porchlight/bin/porchlight", false, settings, conflict)
            assertNotNull(conflict.value) // OFF collector must not immediately erase the user-visible reason
            assertFalse(Files.exists(dir.resolve("autostart/porchlight.desktop")))
            job.cancelAndJoin(); job = SupervisorJob()
            assertFalse(store().settings.first().stayConnected)
        } finally { job.cancelAndJoin(); dir.toFile().deleteRecursively() }
    }
}
