package app.porchlight.shared

import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import java.util.concurrent.*
import kotlin.test.*

class PrerolloutDesktopShutdownTest {
    @Test fun quitLeavesEdtImmediatelyAndHoldsSingletonWhileCleanupIsBlocked() {
        val root = java.nio.file.Files.createTempDirectory(java.nio.file.Files.createDirectories(java.nio.file.Path.of(System.getProperty("porchlight.test.dir"))), "quit-lock-")
        val release = CountDownLatch(1)
        val finished = CompletableFuture<Boolean>()
        val old = checkNotNull(DesktopInstance.acquire(root))
        val shutdown = DesktopShutdown(cleanup = {
            check(!java.awt.EventQueue.isDispatchThread())
            release.await(10, TimeUnit.SECONDS)
        }, releaseInstance = old::close, finish = {
            finished.complete(DesktopInstance.acquire(root)?.use { true } == true)
        }, hardStop = { finished.completeExceptionally(AssertionError("Unexpected hard stop")) })
        try {
            val started = System.nanoTime()
            java.awt.EventQueue.invokeAndWait { shutdown.request() }
            assertTrue(System.nanoTime() - started < 500_000_000L, "Quit must not wait for cleanup on EDT")
            assertFalse(finished.get(4, TimeUnit.SECONDS), "Replacement must not acquire storage ownership while the old cleanup is blocked")
        } finally { release.countDown(); old.close(); root.toFile().deleteRecursively() }
    }
    @Test fun stuckNotificationDrainAndTeardownCannotOutlastQuitBudget() = runBlocking {
        val release = CountDownLatch(1)
        val posting = AuditFinalClientCNotifyBus().apply { holdAll = true }
        val bus = object : DesktopSessionBus by posting, AsyncDesktopSessionBus {
            override fun notifyAsync(owner: String, replaces: Long, title: String, body: String, actions: List<String>) = posting.notifyAsync(owner, replaces, title, body, actions)
            override fun close() { release.await(10, TimeUnit.SECONDS) }
        }
        val f = AuditFinalClientCNotifyRuntime(bus)
        val worker = Executors.newSingleThreadExecutor()
        try {
            f.start(); f.install(); withTimeout(5_000) { posting.first.await() }
            val closed = worker.submit { f.live.close() }
            assertNotNull(runCatching { closed.get(3, TimeUnit.SECONDS); true }.getOrNull(), "Quit must finish within 3 seconds even with a stuck drain and bus teardown")
        } finally {
            release.countDown(); posting.reply.complete(1); worker.shutdownNow(); f.close()
        }
        Unit
    }
}
