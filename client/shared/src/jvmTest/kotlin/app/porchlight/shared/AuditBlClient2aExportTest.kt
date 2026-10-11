package app.porchlight.shared

import app.porchlight.shared.data.ExportSink
import app.porchlight.shared.screens.planning.PendingUiExport
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditBlClient2aExportTest {
    private val sink = object : ExportSink {
        override suspend fun write(bytes: ByteArray) = Unit
        override suspend fun finish(checkGate: () -> Unit) = checkGate()
        override suspend fun abort() = Unit
    }
    @Test fun pickerDeadlineUsesInjectedMonotonicTime() = runTest {
        val clock = kotlin.time.TestTimeSource()
        val pending = PendingUiExport(sink, null, clock)
        clock += kotlin.time.Duration.parse("60s")
        assertFalse(pending.begin(), "A destination returned at the deadline must not start a download")
        assertTrue(pending.result.isCancelled)
        val fresh = PendingUiExport(sink, null, clock)
        clock += kotlin.time.Duration.parse("59s")
        assertTrue(fresh.begin())
        assertFalse(fresh.begin(), "A destination starts only once")
    }
    @Test fun cancellingPendingExportStopsItsDownloader() = runTest {
        val pending = PendingUiExport(sink, null)
        var aborted = false
        val download = backgroundScope.launch(start = CoroutineStart.UNDISPATCHED) {
            assertTrue(pending.begin(currentCoroutineContext().job))
            try { awaitCancellation() } finally { aborted = true }
        }
        pending.cancel()
        runCurrent()
        assertTrue(download.isCancelled, "Downloader kept running after pending export cancellation")
        assertTrue(aborted, "Downloader cleanup must run")
        assertTrue(pending.result.isCancelled)
    }
}
