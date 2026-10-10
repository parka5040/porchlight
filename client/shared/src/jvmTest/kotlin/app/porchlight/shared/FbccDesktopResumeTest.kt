package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.runTest
import java.nio.file.*
import kotlin.test.*
import kotlin.time.Instant

/** fbcc fix 1: the existing logind resume monitor also publishes system wakes for screens (no second bus connection). */
class FbccDesktopResumeTest {
    private class FakeSleepBus : DesktopSleepBus {
        val watching = CompletableDeferred<(Boolean) -> Unit>()
        override fun watch(receive: (Boolean) -> Unit) { watching.complete(receive) }
        override fun close() {}
    }

    @Test fun logindWakeBumpsSystemResumesThroughTheExistingMonitor() = runTest {
        val dir = Files.createTempDirectory(Path.of(System.getProperty("porchlight.test.dir")), "fbcc-resume-")
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall({ error("No network") }, { null }), { now })
        val bus = FakeSleepBus()
        var connections = 0
        val resumes = MutableStateFlow(0L)
        var runtime: DesktopLive? = null
        try {
            runtime = DesktopLive(data, dir, backgroundScope, { now }, { _, _ -> }, {}, MutableStateFlow(null),
                sessionFactory = { FakeSessionBus() }, sleepBusFactory = { connections++; bus }, systemResumes = resumes)
            val signal = bus.watching.await()
            signal(true)
            assertEquals(0L, resumes.value, "going to sleep is not a wake")
            signal(false)
            assertEquals(1L, resumes.value)
            signal(false)
            assertEquals(2L, resumes.value)
            assertEquals(1, connections, "reuses the single system-bus connection")
        } finally { runtime?.close(); data.close(); dir.toFile().deleteRecursively() }
    }
}
