package app.porchlight.shared

import app.porchlight.shared.live.*
import app.porchlight.shared.data.*
import app.porchlight.shared.api.models.ScheduledReminderRead
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.*
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.days

@OptIn(ExperimentalCoroutinesApi::class)
class DesktopFix1Test {
    private fun directory() = Files.createTempDirectory(Path.of(System.getProperty("porchlight.test.dir")), "fix1-")
    @Test fun packagedLauncherDoesNotRequireDeveloperProperty() {
        assertEquals("/opt/porchlight/bin/porchlight", resolveDesktopLauncher("/opt/porchlight/bin/porchlight", "/usr/bin/java"))
        assertEquals("/opt/porchlight/bin/porchlight", resolveDesktopLauncher(null, "/opt/porchlight/bin/porchlight"))
        assertNull(resolveDesktopLauncher(null, "/usr/bin/java"))
    }
    @Test fun focusRequestsWakeFileWatcherWithoutPolling() = runTest {
        val dir = directory(); val first = DesktopInstance.acquire(dir)!!
        try {
            val focused = async(Dispatchers.IO) { first.awaitFocusRequest() }
            assertNull(DesktopInstance.acquire(dir))
            withContext(Dispatchers.IO) { withTimeout(5000) { assertTrue(focused.await()) } }
        } finally { first.close(); dir.toFile().deleteRecursively() }
    }
    @Test fun idleRuntimeUsesNoOwnerPollingAndFutureSnoozeDoesNotFireEarly() = runTest {
        val dir = directory(); val base = Instant.parse("2026-10-06T12:00:00Z")
        val now = { base + kotlin.time.Duration.parse("${testScheduler.currentTime}ms") }
        val cache = memoryCache(now)
        val data = HouseholdData(cache, GeneratedCall({ error("No network") }, { null }), now)
        val fake = FakeSessionBus()
        var runtime: DesktopLive? = null
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("o", "one")),
                scheduledReminders = listOf(ScheduledReminderRead("future", "h", "self", "o", "one", "offset", base + 1.days, base))))
            Files.writeString(dir.resolve("os-delivery.json"), "damaged")
            runtime = DesktopLive(data, dir, backgroundScope, now, { _, _ -> }, {}, MutableStateFlow(null), sleepBusFactory = { NoSleepBus }, sessionFactory = { fake })
            runCurrent()
            data.localSnooze!!("o") // initialization must not hang on corrupt delivery inventory
            val calls = fake.ownerCalls
            val files = Files.list(dir.resolve("alarm-tickets")).use { it.toList().associateWith { p -> Files.readAttributes(p, java.nio.file.attribute.BasicFileAttributes::class.java).fileKey() } }
            advanceTimeBy(60_000); runCurrent()
            assertEquals(calls, fake.ownerCalls)
            assertEquals(0, fake.posts.value)
            files.forEach { (p, key) -> assertEquals(key, Files.readAttributes(p, java.nio.file.attribute.BasicFileAttributes::class.java).fileKey()) }
            advanceTimeBy(86_340_001); runCurrent()
            assertEquals(1, fake.posts.value)
            assertEquals(setOf("o"), data.firedOccurrences.value)
        } finally { runtime?.close(); data.close(); dir.toFile().deleteRecursively() }
    }
    @Test fun suspendWallClockJumpDeliversOverdueAlarmDespiteCacheReconciliation() = runTest {
        val dir = directory(); val base = Instant.parse("2026-10-06T12:00:00Z")
        var wall = base
        val alarmWaitingUntil = MutableStateFlow<Instant?>(null)
        val cache = memoryCache { wall }
        val data = HouseholdData(cache, GeneratedCall({ error("No network") }, { null }), { wall })
        val fake = FakeSessionBus()
        var runtime: DesktopLive? = null
        try {
            val snapshot = emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("o", "one")),
                scheduledReminders = listOf(ScheduledReminderRead("future", "h", "self", "o", "one", "offset", base + 1.days, base)))
            cache.install(snapshot)
            runtime = DesktopLive(data, dir, backgroundScope, { wall }, { _, _ -> }, {}, MutableStateFlow(null), sleepBusFactory = { NoSleepBus }, sessionFactory = { fake }, onAlarmWait = { alarmWaitingUntil.value = it })
            data.localSnooze!!("o") // await durable runtime initialization (no ticket is due yet)
            alarmWaitingUntil.first { it == base + 1.days } // prove the future wait is armed before suspend
            runCurrent()
            wall = base + 1.days + kotlin.time.Duration.parse("5m") // suspend advances wall, not coroutine time
            // Do not await Default-dispatcher disk work here: runTest would auto-advance an unbounded old timer.
            backgroundScope.launch { cache.install(snapshot.copy(cursor = 2)) }; runCurrent()
            advanceTimeBy(60_001); runCurrent()
            assertEquals(1, fake.posts.value)
            assertEquals(setOf("o"), data.firedOccurrences.value)
        } finally { runtime?.close(); data.close(); dir.toFile().deleteRecursively() }
    }
    @Test fun noticesDoNotTurnCloseToTrayIntoQuit() {
        assertTrue(desktopCloseHides(true))
        assertFalse(desktopCloseHides(false))
    }
    @Test fun runtimeNeverDuplicatesInstallerNiriStartup() {
        val dir = directory()
        try {
            Files.createDirectories(dir.resolve("niri"))
            Files.writeString(dir.resolve("niri/config.kdl"), niriAutostart("/opt/porchlight/bin/porchlight"))
            assertEquals(DesktopAutostartResult.NiriManaged, writeDesktopAutostart(dir, "/opt/porchlight/bin/porchlight", true))
            assertFalse(Files.exists(dir.resolve("autostart/porchlight.desktop")))
            Files.delete(dir.resolve("niri/config.kdl"))
            writeDesktopAutostart(dir, "/opt/porchlight/bin/porchlight", true)
            assertTrue(Files.exists(dir.resolve("autostart/porchlight.desktop")))
            writeDesktopAutostart(dir, "/opt/porchlight/bin/porchlight", false)
            assertFalse(Files.exists(dir.resolve("autostart/porchlight.desktop")))
        } finally { dir.toFile().deleteRecursively() }
    }
    @Test fun damagedRowStoreQuarantinesAndNeverReusesCodesOnRestart() {
        val dir = directory()
        try {
            val store = DesktopAlarmStore(dir); val ledger = store.load()
            repeat(4) { ledger.schedule("r$it", Instant.fromEpochMilliseconds(0), "i", "o") }
            store.save(ledger)
            Files.writeString(dir.resolve("alarm-tickets/4.json"), "")
            Files.writeString(dir.resolve("alarm-tickets/next-code"), "damaged")
            val recovered = store.load()
            assertTrue(store.recoveredDamage); assertEquals(5, recovered.nextCode)
            assertTrue(recovered.all().isEmpty())
            val next = recovered.schedule("new", Instant.fromEpochMilliseconds(0), "i", "o")
            store.save(recovered)
            assertEquals(5, next.code)
            assertEquals(next, DesktopAlarmStore(dir).load().all().single())
            assertEquals(1L, Files.list(dir).use { files -> files.filter { it.fileName.toString().startsWith("alarm-tickets.damaged-") }.count() })
        } finally { dir.toFile().deleteRecursively() }
    }
    @Test fun closedFocusWatcherReturnsFalseAndUsesDedicatedDirectory() = runTest {
        val dir = directory(); val instance = DesktopInstance.acquire(dir)!!
        try {
            assertTrue(Files.isDirectory(dir.resolve("focus")))
            instance.close()
            assertFalse(instance.awaitFocusRequest())
        } finally { dir.toFile().deleteRecursively() }
    }
    @Test fun niriDollarAndBacktickAreLiteralNotInvalidKdlEscapes() {
        val path = "/opt/a\$b`c/bin/porchlight"
        assertEquals("spawn-at-startup \"$path\" \"--tray\"", niriAutostart(path))
    }
    @Test fun trayPublishesBrandedArgbPixmap() {
        val icon = porchlightTrayPixmap()
        assertEquals(32, icon.width); assertEquals(32 * 32 * 4, icon.data.size)
        assertTrue(icon.data.any { it.toInt() != 0 })
    }
    @Test fun rowStorePrunesWithoutReusingCodesOrRewritingUnchangedTicket() {
        val dir = directory()
        try {
            val store = DesktopAlarmStore(dir); val ledger = store.load(); val at = Instant.fromEpochMilliseconds(0)
            val first = ledger.schedule("one", at, "i", "o"); val second = ledger.schedule("two", at + 9.days, "i", "o")
            store.save(ledger)
            val file = dir.resolve("alarm-tickets/${second.code}.json")
            val key = Files.readAttributes(file, java.nio.file.attribute.BasicFileAttributes::class.java).fileKey()
            ledger.cancel(first.key); ledger.prune(at + 8.days); store.save(ledger)
            assertEquals(key, Files.readAttributes(file, java.nio.file.attribute.BasicFileAttributes::class.java).fileKey())
            val restored = store.load()
            assertNull(restored.ticket(first.code))
            assertTrue(restored.schedule("one", at + 9.days, "i", "o").code > second.code)
        } finally { dir.toFile().deleteRecursively() }
    }
}
