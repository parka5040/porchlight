package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlinx.serialization.json.*
import java.nio.file.Files
import kotlin.test.*
import kotlin.time.TimeSource

class AuditFinalClientCShutdownTest {
    private fun stopWithWatchdog(f: AuditFinalClientCNotifyRuntime) {
        val executor = java.util.concurrent.Executors.newSingleThreadExecutor { task -> Thread(task, "test-close-watchdog").apply { isDaemon = true } }
        try { executor.submit { f.stop() }.get(4, java.util.concurrent.TimeUnit.SECONDS) }
        finally { executor.shutdownNow() }
    }
    @Test fun readyReplySurvivesShutdownContentionWithBoundedWithdrawal() = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { hold = true; holdNumber = 2 }
        AuditFinalClientCNotifyRuntime(bus).use { f ->
            try {
                f.start(); f.install()
                f.eventually("First alert must have a durable handle") { f.records().singleOrNull()?.handle == 1L }
                val second = notificationFixture().copy(id = "second-alert")
                f.cache.replaceNotifications(listOf(notificationFixture(), second))
                f.eventually("Second Notify must be pending") { bus.attempts.get() == 2 }
                val entered = java.util.concurrent.CountDownLatch(1)
                bus.beforeDismiss = { entered.countDown(); Thread.sleep(1_900) }
                f.cache.replaceNotifications(listOf(second))
                assertTrue(entered.await(3, java.util.concurrent.TimeUnit.SECONDS), "Withdrawal must hold the runtime mutex")
                val arrival = launch(Dispatchers.Default) { delay(150); bus.reply.complete(2L) }
                stopWithWatchdog(f)
                arrival.join()
                assertEquals(2L, f.records().single().handle, "Reply received during a bounded withdrawal must reach disk before close returns")
                bus.beforeDismiss = {}
                f.start(); f.controller.logout()
                f.eventually("Restart logout must withdraw the shutdown-learned handle") { 2L in bus.withdrawn }
                assertEquals(2, bus.attempts.get())
            } finally { bus.beforeDismiss = {}; bus.reply.complete(2L) }
        }
    }
    @Test fun closeDrainsLateReplyDurablyThenRestartLogoutWithdrawsIt() = drain(false)
    @Test fun closeDrainsRetiredReplyIntoDurableWithdrawOnlyTombstone() = drain(true)
    private fun drain(retire: Boolean) = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { hold = true }
        AuditFinalClientCNotifyRuntime(bus).use { f ->
            f.start(); f.install()
            withTimeout(5_000) { bus.first.await() }
            delay(2_100) // already past the local bound; only the completion owner can record this reply
            if (retire) f.controller.logout()
            val arrival = launch(Dispatchers.Default) { delay(150); bus.reply.complete(1L) }
            stopWithWatchdog(f) // mirrors production: live closes BEFORE cancelling its service scope or closing cache
            arrival.join()
            if (retire) {
                val saved = WireJson.parseToJsonElement(Files.readString(f.dir.resolve("os-delivery.json"))).jsonObject
                assertEquals(1L, saved["withdrawals"]?.jsonArray?.singleOrNull()?.jsonObject?.get("handle")?.jsonPrimitive?.long,
                    "Retired late handle must be a durable withdrawal before bus close")
                assertTrue(f.records().isEmpty())
            } else assertEquals(1L, f.records().single().handle, "Normal close must persist a late handle before cancelling completion work")
            assertTrue(bus.busClosed)
            f.start()
            if (!retire) f.controller.logout()
            f.eventually("New runtime must withdraw the handle learned during shutdown") { 1L in bus.withdrawn }
            assertEquals(1, bus.posts)
        }
    }
    @Test fun closeHasOneBoundForAllRepliesThatNeverArrive() = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { holdAll = true }
        AuditFinalClientCNotifyRuntime(bus).use { f ->
            f.start()
            f.cache.install(emptySnapshot(1).copy(notifications = (1..3).map { notificationFixture().copy(id = "alert-$it") }))
            f.eventually("All replies must be pending before shutdown") { bus.attempts.get() == 3 }
            val started = TimeSource.Monotonic.markNow()
            stopWithWatchdog(f)
            assertTrue(started.elapsedNow().inWholeMilliseconds < 2_900, "Close must use one 2.5-second drain deadline, with scheduling allowance")
            assertTrue(bus.busClosed)
            assertEquals(3, f.records().size)
            assertTrue(f.records().all { it.inFlight && it.handle == 0L })
        }
    }
    @Test fun unresolvedRestartRowIsNotRepostedAndIdentityChangeDropsItWithoutBlockingAdmission() = runBlocking {
        val bus = AuditFinalClientCNotifyBus().apply { hold = true }
        AuditFinalClientCNotifyRuntime(bus).use { f ->
            f.start(); f.install(); withTimeout(5_000) { bus.first.await() }; delay(2_100)
            stopWithWatchdog(f); f.start(); delay(100)
            assertEquals(1, bus.attempts.get(), "Same-daemon unresolved row must not be blindly resent after restart")
            f.fixture.saved = checkNotNull(f.fixture.saved).withIdentity("second", emptyList())
            f.controller.restore()
            withTimeout(5_000) { f.controller.liveAdmission.first { it != null } }
            assertTrue(f.records().isEmpty(), "Unknown handle cannot be withdrawn: retire the unresolved old-identity row")
            assertTrue(bus.withdrawn.isEmpty(), "Never invent a daemon handle")
            f.cache.install(emptySnapshot(2).copy(notifications = listOf(notificationFixture().copy(id = "replacement-alert"))))
            f.eventually("Unresolved old row must not block replacement admission") { bus.posts == 2 }
            assertEquals(2, bus.attempts.get())
        }
    }
}
