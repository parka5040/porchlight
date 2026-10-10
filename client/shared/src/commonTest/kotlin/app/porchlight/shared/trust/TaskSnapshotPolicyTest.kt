package app.porchlight.shared.trust

import kotlin.test.*

class TaskSnapshotPolicyTest {
    private class Window : SnapshotWindow {
        var secured = false
        var recentsDisabled = false
        override fun disableRecentsScreenshots() { recentsDisabled = true }
        override fun setSecure(secure: Boolean) { this.secured = secure }
    }
    @Test fun lifecycleSecuresPausedAndStoppedWindowsOnEveryApiAndAllowsForegroundScreenshots() {
        for (api in listOf(30, 32, 33, 35, 36)) {
            val window = Window(); val policy = TaskSnapshotPolicy(api)
            policy.apply(SnapshotLifecycle.CREATED, window)
            assertTrue(window.secured)
            assertEquals(api >= 33, window.recentsDisabled)
            policy.apply(SnapshotLifecycle.RESUMED, window)
            assertFalse(window.secured)
            policy.apply(SnapshotLifecycle.PAUSED, window) // picker sheet or permission dialog: no ON_STOP
            assertTrue(window.secured)
            policy.apply(SnapshotLifecycle.STOPPED, window)
            assertTrue(window.secured)
            policy.apply(SnapshotLifecycle.RESUMED, window)
            assertFalse(window.secured)
        }
    }
}
