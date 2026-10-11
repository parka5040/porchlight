package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import java.nio.file.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditFinalClientCAlarmDeleteTest {
    @Test fun failedDeletionKeepsConcreteFileObligationUntilRetryAndReopen() = runTest {
        val f = AuditFinalClientCFixture()
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "alarm-delete-")
        val store = DesktopAlarmStore(dir); val ledger = store.load()
        ledger.schedule("old-reminder", f.now, "item", "occ"); val ticket = ledger.snooze("old-reminder", f.now)
        store.save(ledger)
        val file = dir.resolve("alarm-tickets/${ticket.code}.json")
        val saved = dir.resolve("saved-ticket")
        Files.move(file, saved); Files.createDirectory(file); Files.writeString(file.resolve("busy"), "busy")
        try {
            ledger.retire()
            assertFailsWith<DirectoryNotEmptyException> { store.save(ledger) }
            Files.delete(file.resolve("busy")); Files.delete(file); Files.move(saved, file)
            store.save(ledger)
            assertFalse(Files.exists(file), "A successful retry must actually delete the old snoozed ticket file")
            val reopened = DesktopAlarmStore(dir).load()
            assertTrue(reopened.all().isEmpty())
            assertTrue(reopened.nextCode > ticket.code, "Retirement must preserve allocation high-water")
            val cache = memoryCache { f.now }; val bus = FakeSessionBus()
            val controller = f.controller(StandardTestDispatcher(testScheduler))
            val data = HouseholdData(cache, GeneratedCall.trusted(controller), { f.now })
            val live = DesktopLive(data, dir, backgroundScope, { f.now }, { _, _ -> }, {}, MutableStateFlow(null), controller,
                sessionFactory = { bus }, sleepBusFactory = { NoSleepBus })
            data.bindTrust(controller, backgroundScope, onIdentityEnded = { live.clearIdentity(it) },
                onIdentityStarting = { owner, epoch -> live.prepareIdentity(owner, epoch) },
                streamEnabled = MutableStateFlow(false), refreshWhenInactive = false)
            try {
                f.saved = checkNotNull(f.saved).withIdentity("second", emptyList()); controller.restore()
                cache.install(emptySnapshot(2).copy(items = listOf(itemRead("item")), occurrences = listOf(occurrenceRead("occ", "item"))))
                runCurrent(); assertEquals(0, bus.posts.value, "Replacement identity must not deliver the retired snooze")
            } finally { live.close(); controller.close(); cache.close(); runCurrent() }
        } finally { dir.toFile().deleteRecursively() }
    }
}
