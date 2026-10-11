package app.porchlight.shared

import app.porchlight.shared.backups.*
import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalCoroutinesApi::class)
class AuditBlClientDesktopLiveTest {
    @Test fun healthyBusClearsOnlyBusNoticeWhenAlarmRecoveryFailed() = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "bl-live-")
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall({ error("No network") }, { null }), { now })
        val notice = MutableStateFlow<String?>(null)
        val session = FakeSessionBus().apply { watcher = false }
        var live: DesktopLive? = null
        try {
            cache.install(emptySnapshot(1))
            live = DesktopLive(data, root, backgroundScope, { now }, { _, _ -> }, {}, notice,
                sessionFactory = { session }, alarmLoader = { error("recovery failed") }, sleepBusFactory = { NoSleepBus })
            data.localSnooze!!("o"); runCurrent()
            assertTrue(notice.value.orEmpty().contains("inventory recovery failed"))
            assertTrue(notice.value.orEmpty().contains("Desktop tray/notifications unavailable"))
            session.watcher = true; session.listener!!.watcherChanged(); runCurrent()
            assertTrue(notice.value.orEmpty().contains("inventory recovery failed"))
            assertFalse(notice.value.orEmpty().contains("Desktop tray/notifications unavailable"), "Recovered bus must clear its own reason")
        } finally { live?.close(); data.close(); root.toFile().deleteRecursively() }
    }

    @Test fun identicalBackupMenuProjectionDoesNotEmitAnotherRegistration() {
        val actions = DesktopTrayActions()
        var changes = 0
        actions.changed = { changes++ }
        actions.registerBackupStatus(BackupView(allowed = true, localBundle = "one.tar.age"))
        actions.registerBackupStatus(BackupView(allowed = true, localBundle = "two.tar.age"))
        assertEquals(1, changes, "Non-visible backup state changes must not rebuild the menu")
        actions.registerBackupStatus(BackupView(allowed = true, pulling = true))
        assertEquals(2, changes)
        actions.registerBackupStatus(BackupView()); actions.registerBackupStatus(BackupView())
        assertEquals(3, changes)
    }
}
