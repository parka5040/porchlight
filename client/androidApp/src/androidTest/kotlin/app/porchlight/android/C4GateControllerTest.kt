package app.porchlight.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.android.trust.ControllerOnly
import app.porchlight.shared.data.withBackgroundSession
import app.porchlight.shared.settings.AndroidAppServices
import app.porchlight.shared.trust.TrustState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import org.json.JSONObject
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Disposable fresh AVD only. Controller transfers secrets over stdin, never instrumentation argv. */
@ControllerOnly
@RunWith(AndroidJUnit4::class)
class C4GateControllerTest {
    @Test fun enrollGateDeviceAndKeepPinLockedBackgroundSession() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val input = File(context.filesDir, "m5-enroll.json")
        check(input.exists()) { "Required private M5 enrollment input missing" }
        val config = try { JSONObject(input.readText()) } finally { input.delete() }
        runBlocking(Dispatchers.IO) {
            withTimeout(60_000) {
                val services = AndroidAppServices.get(context)
                val controller = services.trust.controller
                controller.ensureRestored()
                controller.state.first { it != TrustState.Loading }
                check(controller.state.value == TrustState.Enrollment) { "M5 requires a fresh unenrolled AVD" }
                controller.enroll(config.getString("uri"), config.getString("password"))
                check(controller.state.value == TrustState.PinRequired)
                services.data.cache.initialize()
                withBackgroundSession(controller) { services.data.sync.refresh() }
                check(services.data.cache.view.value.sync != null)
                services.settings.setStayConnected(true) // opt-in on this disposable gate profile only
                check(controller.state.value == TrustState.PinRequired) { "Background sync must not unlock UI" }
            }
        }
    }
}
