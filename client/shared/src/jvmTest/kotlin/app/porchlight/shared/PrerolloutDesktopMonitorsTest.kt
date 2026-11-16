package app.porchlight.shared

import app.porchlight.shared.theme.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutDesktopMonitorsTest {
    @Test fun resumeMonitorRecoversAbsentBusAndReconnectsAfterLossWithoutHealthyPolling() = runTest {
        var attempts = 0; var resumes = 0; var closed = 0
        var sleep: ((Boolean) -> Unit)? = null
        var lost: (() -> Unit)? = null
        val monitor = app.porchlight.shared.live.DesktopResumeMonitor({ resumes++ }, backgroundScope) {
            attempts++
            if (attempts == 1) error("System bus unavailable")
            object : app.porchlight.shared.live.DesktopSleepBus {
                override fun watch(receive: (Boolean) -> Unit) { sleep = receive }
                override fun watchLoss(onLoss: () -> Unit) { lost = onLoss }
                override fun close() { closed++ }
            }
        }
        try {
            runCurrent(); advanceTimeBy(1_001); runCurrent()
            assertEquals(2, attempts)
            sleep!!(false); assertEquals(1, resumes)
            advanceTimeBy(3_600_000); runCurrent(); assertEquals(2, attempts)
            lost!!(); runCurrent(); advanceTimeBy(1_001); runCurrent()
            assertEquals(3, attempts)
            sleep!!(true); assertEquals(1, resumes)
            sleep!!(false); assertEquals(2, resumes)
        } finally { monitor.close(); runCurrent() }
        assertTrue(closed >= 2)
    }
    @Test fun appearanceRecoversWhenSessionPortalAppearsAfterAutostart() = runTest {
        var attempts = 0
        val source = PortalAppearanceSource { flow { if (++attempts == 1) error("No session bus yet"); emit(2); awaitCancellation() } }
        val theme = DesktopPortalTheme(backgroundScope, source, StandardTestDispatcher(testScheduler))
        runCurrent(); assertTrue(theme.dark.value)
        advanceTimeBy(60_000); runCurrent()
        assertFalse(theme.dark.value, "A transient portal failure must recover without restarting the app")
        assertEquals(2, attempts)
    }
}
