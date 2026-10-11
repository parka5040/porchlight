package app.porchlight.shared.files

import java.io.File
import java.nio.file.Files
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.*
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AuditBlClient3aFix1DesktopCancellationTest {
    @Test fun cancellationAfterAllocationDeletesSpoolWithoutPublishingDestination() = runTest {
        val root = File(System.getProperty("porchlight.test.dir"), "export-cancellation-audit").apply { mkdirs() }
        val dir = Files.createTempDirectory(root.toPath(), "io-").toFile()
        val owner = CoroutineScope(SupervisorJob(coroutineContext[Job]) + StandardTestDispatcher(testScheduler))
        val allocated = AtomicReference<File>()
        var writeStarted = false
        var warningReported = false
        var resultReported = false
        val destination = File(dir, "member.json")
        try {
            val files = DesktopCalendarFiles(owner, { _, _ -> destination }, {
                File.createTempFile("spool-", ".part", dir).also {
                    allocated.set(it)
                    // Deterministically cancel inside IO, before the allocation result can be dispatched back.
                    owner.cancel("Identity changed after spool allocation")
                }
            })
            files.save(false, { writeStarted = true }, { warningReported = true }, { resultReported = true })
            owner.coroutineContext.job.children.single().join()
            assertNotNull(allocated.get(), "The test must reach spool allocation")
            assertFalse(writeStarted)
            assertFalse(warningReported)
            assertFalse(resultReported)
            assertFalse(destination.exists(), "Cancelled export must not publish a destination")
            assertFalse(allocated.get().exists(), "Cancelled allocation must not leave an orphan .part spool")
            assertTrue(dir.listFiles().orEmpty().isEmpty())
        } finally { owner.cancel(); dir.deleteRecursively() }
    }
}
