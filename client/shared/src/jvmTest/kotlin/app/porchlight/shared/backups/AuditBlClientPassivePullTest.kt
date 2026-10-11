package app.porchlight.shared.backups

import app.porchlight.shared.api.models.BackupStatus
import app.porchlight.shared.api.models.Bundle
import app.porchlight.shared.transport.ConnectFailure
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import java.nio.file.Files
import java.nio.file.Path
import java.time.ZoneId
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.seconds

@OptIn(ExperimentalCoroutinesApi::class)
class AuditBlClientPassivePullTest {
    private class Source : BackupSource {
        var offline = false
        var statusFailure = false
        var calls = 0
        var onStatus: suspend () -> Unit = {}
        override suspend fun authorizedHousehold(): String? {
            if (offline) throw ConnectFailure()
            return "household"
        }
        override fun ensureAuthorized() {}
        override suspend fun status(): BackupStatus {
            calls++; onStatus()
            if (statusFailure) error("storage unavailable")
            return BackupStatus()
        }
        override suspend fun download(bundle: Bundle, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) = error("No bundle")
    }
    private suspend fun fixture(test: suspend (Path) -> Unit) {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "bl-pull-")
        try { test(root) } finally { root.toFile().deleteRecursively() }
    }

    @Test fun recoveredNonDueReachabilityResetsPriorOutageBackoff() = runTest { fixture { root ->
        var now = Instant.parse("2026-10-06T05:00:00Z")
        val source = Source()
        val pull = PassivePull(root, source, { now }, zone = { ZoneId.of("UTC") }, jitter = { 1.0 })
        pull.tick(); assertEquals(1, source.calls)
        source.offline = true
        repeat(4) { now += 60.seconds; pull.tick() }
        source.offline = false
        pull.tick() // recovered, but the nightly pull is not due
        assertFalse(pull.state.value.cantReachHome)
        assertEquals(1, source.calls)
        source.statusFailure = true
        pull.pull() // a new failure must start from one minute, not the earlier outage's backoff
        assertEquals(2, source.calls)
        now += 60.seconds
        pull.tick()
        assertEquals(3, source.calls, "New failure retries after one minute")
    } }

    @Test fun resetWaitsForPullAndCannotBeOverwrittenByItsCompletion() = runTest { fixture { root ->
        val source = Source()
        val entered = CompletableDeferred<Unit>()
        val release = CompletableDeferred<Unit>()
        source.onStatus = { entered.complete(Unit); release.await() }
        val pull = PassivePull(root, source, { Instant.parse("2026-10-06T05:00:00Z") })
        val active = launch { pull.pull() }
        var reset: Job? = null
        try {
            entered.await()
            reset = launch { pull.resetView() }
            runCurrent()
            assertFalse(reset.isCompleted, "Reset must serialize with the in-flight pull")
            release.complete(Unit); active.join(); reset.join()
            assertEquals(BackupView(), pull.state.value)
        } finally { release.complete(Unit); active.cancelAndJoin(); reset?.cancelAndJoin() }
    } }
}
