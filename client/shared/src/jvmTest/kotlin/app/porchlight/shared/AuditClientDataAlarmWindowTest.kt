package app.porchlight.shared

import app.porchlight.shared.api.models.ScheduledReminderRead
import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

@OptIn(ExperimentalCoroutinesApi::class)
class AuditClientDataAlarmWindowTest {
    @Test fun silentCacheArmsFutureReminderAndResumeDeliversExactlyOnce() = runTest {
        val base = Instant.parse("2026-10-06T12:00:00Z")
        var wall = base
        val waits = mutableListOf<Instant?>()
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-window-")
        val cache = memoryCache { wall }
        val data = HouseholdData(cache, GeneratedCall({ error("Offline") }, { null }), { wall })
        val session = FakeSessionBus()
        var resume: (Boolean) -> Unit = {}
        val sleep = object : DesktopSleepBus {
            override fun watch(receive: (Boolean) -> Unit) { resume = receive }
            override fun close() = Unit
        }
        var runtime: DesktopLive? = null
        try {
            val due = base + 8.days
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("i")),
                occurrences = listOf(occurrenceRead("o", "i").copy(status = "backlog")),
                scheduledReminders = listOf(ScheduledReminderRead("r", "h", "self", "o", "i", "offset", due, base))))
            runtime = DesktopLive(data, dir, backgroundScope, { wall }, { _, _ -> }, {}, MutableStateFlow(null),
                sessionFactory = { session }, sleepBusFactory = { sleep }, onAlarmWait = { waits += it })
            data.localSnooze!!("o"); runCurrent()
            assertEquals(listOf<Instant?>(null), waits)
            wall = base + 2.days
            advanceTimeBy(15 * 60_000L); runCurrent()
            assertEquals(due, waits.last(), "silent rolling window must arm")
            wall = due
            resume(false); runCurrent()
            assertEquals(1, session.posts.value, "resume must deliver backlog immediately")
            advanceTimeBy(15 * 60_000L); runCurrent()
            assertEquals(1, session.posts.value)
        } finally { runtime?.close(); data.close(); dir.toFile().deleteRecursively() }
    }
}
