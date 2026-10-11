package app.porchlight.shared

import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import kotlin.test.*

class AuditFinalClientCRetryTest {
    @Test fun noDaemonDoesNotPrepareWriteOrAttemptServerAlerts() = noDaemon(false)
    @Test fun noDaemonDoesNotPrepareWriteOrAttemptLocalReminders() = noDaemon(true)
    private fun noDaemon(local: Boolean) = runBlocking {
        val session = FakeSessionBus().apply { owner = null }
        val bus = AuditFinalClientCNotifyBus(session)
        val writes = AtomicInteger()
        val retrySleeps = AtomicInteger()
        AuditFinalClientCNotifyRuntime(bus, deliveryWriter = { file, text -> writes.incrementAndGet(); writeDesktopDelivery(file, text) },
            notifyRetryDelay = { retrySleeps.incrementAndGet(); delay(it) }).use { f ->
            f.start(); delay(100) // exclude identity/admission initialization writes
            writes.set(0)
            f.install(local)
            delay(2_200) // exceeds two old retry intervals, even without cache churn
            assertEquals(0, writes.get(), "Absent notification daemon must cause zero delivery inventory writes")
            assertEquals(0, retrySleeps.get(), "Absent daemon must not schedule a polling retry")
            assertEquals(0, bus.attempts.get())
            assertTrue(f.records().isEmpty(), "No posting record should be prepared without a daemon")
            session.restart(":1.20")
            f.eventually("Daemon appearance must wake delivery without another cache change") { f.records().singleOrNull()?.posted == true }
            assertEquals(1, bus.attempts.get())
            assertEquals(1, bus.posts)
        }
    }
    private class Clock {
        val nanos = AtomicLong(1)
        data class Sleep(val ms: Long, val ready: CompletableDeferred<Unit> = CompletableDeferred())
        val waits = Channel<Sleep>(Channel.UNLIMITED)
        suspend fun delay(ms: Long) { val sleep = Sleep(ms); waits.send(sleep); sleep.ready.await() }
        suspend fun next(expected: Long): Sleep = withTimeout(3_000) { waits.receive() }.also {
            assertEquals(expected, it.ms, "Definite rejection delays must double, capped at five minutes")
        }
        fun advance(sleep: Sleep) { nanos.addAndGet(sleep.ms * 1_000_000); sleep.ready.complete(Unit) }
    }
    @Test fun definiteRejectionsBackOffExponentiallyToFiveMinutes() = runBlocking {
        val clock = Clock()
        val bus = AuditFinalClientCNotifyBus().apply { reject = true }
        AuditFinalClientCNotifyRuntime(bus, monotonicNanos = clock.nanos::get, notifyRetryDelay = clock::delay).use { f ->
            f.start(); f.install()
            val delays = listOf(1_000L, 2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 128_000, 256_000, 300_000, 300_000)
            for ((index, ms) in delays.withIndex()) {
                val sleep = clock.next(ms)
                assertEquals(index + 1, bus.attempts.get())
                repeat(3) { f.cache.replaceNotifications(listOf(notificationFixture())); yield() }
                assertEquals(index + 1, bus.attempts.get(), "Cache churn cannot bypass backoff before the clock advances")
                if (index < delays.lastIndex) clock.advance(sleep)
            }
            assertEquals(0, bus.posts)
        }
    }
    @Test fun successResetsRejectionBackoff() = reset(false)
    @Test fun serverOwnerChangeResetsRejectionBackoff() = reset(true)
    private fun reset(ownerChange: Boolean) = runBlocking {
        val clock = Clock()
        val session = FakeSessionBus()
        val bus = AuditFinalClientCNotifyBus(session).apply { reject = true }
        AuditFinalClientCNotifyRuntime(bus, monotonicNanos = clock.nanos::get, notifyRetryDelay = clock::delay).use { f ->
            f.start(); f.install()
            clock.advance(clock.next(1_000))
            val old = clock.next(2_000)
            if (ownerChange) session.restart(":1.20") else {
                bus.reject = false
                clock.advance(old)
                f.eventually("Healthy attempt must commit before the next rejection") { f.records().singleOrNull()?.posted == true }
                bus.reject = true
                val row = notificationFixture()
                f.cache.replaceNotifications(listOf(row.copy(version = row.version + 1)))
            }
            clock.next(1_000)
            assertEquals(if (ownerChange) 3 else 4, bus.attempts.get())
            assertEquals(if (ownerChange) 0 else 1, bus.posts)
        }
    }
}
