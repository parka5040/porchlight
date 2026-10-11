package app.porchlight.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.android.trust.ControllerOnly
import app.porchlight.shared.data.withBackgroundSession
import app.porchlight.shared.live.DeliveryInventory
import app.porchlight.shared.settings.AndroidAppServices
import app.porchlight.shared.trust.TrustState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import org.json.JSONObject
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@ControllerOnly
@RunWith(AndroidJUnit4::class)
class C4GateDismissControllerTest {
    @Test fun dismissWithOriginalDeviceDeliveryKeyWhileLocked() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val input = File(context.filesDir, "m5-dismiss.json")
        check(input.exists()) { "Required M5 dismissal input missing" }
        val id = try { JSONObject(input.readText()).getString("id") } finally { input.delete() }
        val inventory = DeliveryInventory.decode(app.porchlight.shared.settings.privateMetadata(context, "os-delivery").getString("table", "[]")!!)
        inventory.requirePostedAction(id)
        runBlocking(Dispatchers.IO) {
            withTimeout(60_000) {
                val services = AndroidAppServices.get(context)
                val controller = services.trust.controller
                controller.ensureRestored(); controller.state.first { it != TrustState.Loading }
                check(controller.state.value in setOf(TrustState.PinRequired, TrustState.Ready))
                withBackgroundSession(controller) {
                    services.data.cache.initialize()
                    app.porchlight.shared.live.dismissDeliveredNotification(services.data, inventory, id)
                }
            }
        }
    }
}
