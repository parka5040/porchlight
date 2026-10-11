package app.porchlight.shared.data

import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlinx.coroutines.flow.first
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditBlClient2bCleanupTest {
    @Test fun failedIdentityFileCleanupDoesNotKillBindingAndRetriesWithoutNetwork() = runTest {
        val first = CompletableDeferred<Unit>()
        val cleaned = CompletableDeferred<Unit>()
        var clears = 0
        val store = object : AttachmentStore by MemoryAttachmentStore() {
            override suspend fun clear() {
                if (++clears == 1) { first.complete(Unit); error("Temporary file deletion failure") }
                cleaned.complete(Unit)
            }
        }
        val cache = memoryCache(store) { auditEnrollmentNow }
        val data = HouseholdData(cache, GeneratedCall({ error("No network before cleanup") }, { null }), { auditEnrollmentNow })
        val controller = TrustController(AuditEnrollmentKeys(), AuditEnrollmentFactory(), "desktop", { auditEnrollmentNow }, false, StandardTestDispatcher(testScheduler))
        val failures = mutableListOf<Throwable>()
        val owner = CoroutineScope(SupervisorJob() + StandardTestDispatcher(testScheduler) + CoroutineExceptionHandler { _, failure -> failures += failure })
        try {
            controller.restore()
            val binding = data.bindTrust(controller, owner)
            first.await(); data.sync.status.first { it is SyncStatus.Failed }
            assertTrue(binding.isActive, "A file cleanup error must not terminate the identity collector")
            assertTrue(failures.isEmpty())
            assertEquals(SyncStatus.Failed("attachment-cleanup-failed"), data.sync.status.value)
            advanceTimeBy(1000); runCurrent()
            cleaned.await(); data.sync.status.first { it == SyncStatus.Idle }
            assertTrue(binding.isActive)
            assertEquals(2, clears)
            assertEquals(SyncStatus.Idle, data.sync.status.value)
        } finally { owner.cancel(); controller.close(); data.close() }
    }
}
