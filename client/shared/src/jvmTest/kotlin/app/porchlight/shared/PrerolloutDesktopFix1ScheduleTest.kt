package app.porchlight.shared

import app.porchlight.shared.backups.*
import app.porchlight.shared.api.models.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.*
import java.nio.file.*
import kotlin.time.Instant
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutDesktopFix1ScheduleTest {
    @Test fun wallClockJumpWithoutResumeSignalCatchesUpWithinFiveMinutes() = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "backup-wall-")
        var clock = Instant.parse("2026-10-06T05:00:00Z")
        var attempts = 0
        val source = object : BackupSource {
            override suspend fun authorizedHousehold() = "household"
            override fun ensureAuthorized() = Unit
            override suspend fun status(): BackupStatus { attempts++; return BackupStatus() }
            override suspend fun download(bundle: Bundle, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) = error("No bundle")
        }
        val pull = PassivePull(root, source, { clock }, zone = { java.time.ZoneId.of("UTC") })
        try {
            pull.tick()
            assertNull(pull.state.value.error)
            assertEquals(1, root.toFile().walk().count { it.name == ".next-pull" })
            assertEquals(1, attempts)
            val waiting = java.util.concurrent.CountDownLatch(1)
            pull.ownershipStep = { if (it.startsWith("schedule.wait")) waiting.countDown() }
            val job = pull.start(backgroundScope, MutableStateFlow("household"))
            repeat(100) {
                runCurrent()
                if (waiting.count > 0) Thread.sleep(10)
            }
            assertEquals(0, waiting.count, "Scheduler must have read the persisted deadline before suspend")
            clock = Instant.fromEpochSeconds(clock.epochSeconds + 24 * 3600)
            advanceTimeBy(300_001); runCurrent()
            repeat(100) { if (attempts < 2) { Thread.sleep(10); runCurrent() } }
            assertEquals(2, attempts, "Overdue wall-clock deadline must recover without logind or an overnight monotonic timeout")
            job.cancelAndJoin()
        } finally { root.toFile().deleteRecursively() }
    }
    @Test fun trayAvailabilityIsPendingUntilRegistrationConfirmsOutcome() {
        val services = app.porchlight.shared.settings.DesktopAppServices()
        try { assertNull(services.trayAvailable.value, "Pending registration must not reveal a --tray launch") }
        finally { services.close() }
    }
}
