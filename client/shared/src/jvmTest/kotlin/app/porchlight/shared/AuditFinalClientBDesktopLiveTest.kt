package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientBDesktopLiveTest {
    @Test fun pausedReconciliationCannotRepostAfterLogout() = interleave(false)
    @Test fun pausedReconciliationCannotRepostAfterMemberSwitch() = interleave(true)
    private fun interleave(switch: Boolean) = runTest {
        val fixture = AuditFinalClientBLiveFixture(StandardTestDispatcher(testScheduler))
        val controller = fixture.controller
        val cache = memoryCache { fixture.now }
        val data = HouseholdData(cache, GeneratedCall({ error("No network") }, { null }), { fixture.now })
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "final-live-")
        val bus = FakeSessionBus()
        val paused = CompletableDeferred<Unit>()
        val resume = CompletableDeferred<Unit>()
        var runtime: DesktopLive? = null
        try {
            controller.restore()
            cache.sessionEpoch = { controller.sessionEpoch.value }
            cache.sessionCommitMutex = controller.sessionCommitMutex
            cache.install(emptySnapshot(1).copy(notifications = listOf(notificationFixture()),
                items = listOf(itemRead("item")), occurrences = listOf(occurrenceRead("occ", "item")),
                scheduledReminders = listOf(app.porchlight.shared.api.models.ScheduledReminderRead("reminder", "h", "self", "occ", "item", "offset",
                    fixture.now + kotlin.time.Duration.parse("1h"), fixture.now))))
            runtime = DesktopLive(data, dir, backgroundScope, { fixture.now }, { _, _ -> }, {}, MutableStateFlow(null), controller,
                sessionFactory = { bus }, sleepBusFactory = { NoSleepBus },
                beforeReconcile = { paused.complete(Unit); resume.await() })
            controller.identityEnded = { cache.clear(); runtime.clearIdentity() }
            paused.await()
            if (switch) fixture.switchMember() else controller.logout()
            assertEquals(1, controller.sessionEpoch.value)
            resume.complete(Unit); runCurrent()
            assertEquals(0, bus.posts.value, "Captured old view must not post after identity cleanup")
            assertTrue(DesktopAlarmStore(dir).load().active().isEmpty())
        } finally { resume.complete(Unit); runtime?.close(); controller.close(); cache.close(); dir.toFile().deleteRecursively(); runCurrent() }
    }
}
