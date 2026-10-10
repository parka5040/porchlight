package app.porchlight.android.trust

import android.view.WindowManager
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import app.porchlight.android.ShellFixtureActivity
import app.porchlight.shared.trust.PorchlightApplication
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Real Application callback dispatch/window flags; actual Recents cards need the device checklist. */
@RunWith(AndroidJUnit4::class)
class TaskSnapshotInstrumentedTest {
    @Test fun applicationSecuresPausedAndStoppedActivityAndClearsOnResume() {
        ActivityScenario.launch(ShellFixtureActivity::class.java).use { scenario ->
            fun assertSecure(expected: Boolean) {
                scenario.onActivity {
                    assertTrue(it.application is PorchlightApplication)
                    assertEquals(expected, it.window.attributes.flags and WindowManager.LayoutParams.FLAG_SECURE != 0)
                }
            }
            assertSecure(false)
            scenario.moveToState(Lifecycle.State.STARTED) // paused, not stopped: picker/dialog case
            assertSecure(true)
            scenario.moveToState(Lifecycle.State.CREATED)
            assertSecure(true)
            scenario.moveToState(Lifecycle.State.RESUMED)
            assertSecure(false)
        }
    }
}
