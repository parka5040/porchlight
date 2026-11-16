package app.porchlight.shared.live

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** Quit never drains native replies, closes a database, or waits for D-Bus on AWT's event thread. */
class DesktopShutdown(private val cleanup: () -> Unit, @Suppress("UNUSED_PARAMETER") releaseInstance: () -> Unit,
    private val finish: () -> Unit, private val hardStop: () -> Unit) {
    private val started = AtomicBoolean()
    fun request() {
        if (!started.compareAndSet(false, true)) return
        val cleaned = CountDownLatch(1)
        val finished = CountDownLatch(1)
        Thread({
            try { cleanup() } finally { cleaned.countDown() }
        }, "porchlight-shutdown-cleanup").apply { isDaemon = true; start() }
        Thread({
            cleaned.await(2_900, TimeUnit.MILLISECONDS)
            // Cancellation is not quiescence: a cache/trust/native writer may still be finishing. Retain the
            // singleton fd even after successful cleanup; only process death proves every writer has stopped.
            // The replacement also waits for this PID before trying to acquire storage ownership.
            try { finish() } finally { finished.countDown() }
        }, "porchlight-shutdown").apply { isDaemon = true; start() }
        Thread({
            if (!finished.await(5, TimeUnit.SECONDS)) hardStop()
        }, "porchlight-shutdown-deadline").apply { isDaemon = true; start() }
    }
}
