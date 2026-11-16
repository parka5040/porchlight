package app.porchlight.shared

import app.porchlight.shared.live.*
import kotlinx.coroutines.*
import java.nio.file.*
import java.util.concurrent.*
import kotlin.test.*

class PrerolloutDesktopFix1OwnershipTest {
    @Test fun replacementWaitsForPredecessorDeathEvenIfLockIsAvailable() = runBlocking {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "pid-handoff-")
        val writer = ProcessBuilder("bash", "-c", "read -r _", "writer").start()
        try {
            val replacement = async(Dispatchers.IO) { DesktopInstance.acquire(root, afterUpdate = true, predecessorPid = writer.pid()) }
            delay(250)
            assertFalse(replacement.isCompleted, "A released fd is not evidence that old storage writers have stopped")
            writer.outputStream.close()
            assertTrue(writer.waitFor(3, TimeUnit.SECONDS))
            withTimeout(3_000) { assertNotNull(replacement.await()).close() }
        } finally { writer.destroyForcibly(); writer.waitFor(); root.toFile().deleteRecursively() }
    }
    @Test fun successfulCleanupAlsoKeepsOwnershipThroughProcessExit() {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "exit-ownership-")
        val old = assertNotNull(DesktopInstance.acquire(root))
        val finished = CountDownLatch(1)
        try {
            DesktopShutdown({}, old::close, { finished.countDown() }, { error("Unexpected hard stop") }).request()
            assertTrue(finished.await(3, TimeUnit.SECONDS))
            assertNull(DesktopInstance.acquire(root), "Only process termination proves all independent writer scopes are gone")
        } finally { old.close(); root.toFile().deleteRecursively() }
    }
}
