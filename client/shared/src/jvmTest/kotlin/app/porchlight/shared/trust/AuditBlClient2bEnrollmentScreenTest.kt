package app.porchlight.shared.trust

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runComposeUiTest
import androidx.compose.ui.unit.Density
import app.porchlight.shared.transport.ApiProblem
import kotlinx.coroutines.runBlocking
import kotlin.test.*

@OptIn(ExperimentalTestApi::class)
class AuditBlClient2bEnrollmentScreenTest {
    @Test fun reusedKeyRecoveryCopyIsVisibleInTheRealTrustGate() = runComposeUiTest {
        val keys = AuditEnrollmentKeys()
        val factory = AuditEnrollmentFactory().also { it.failure = ApiProblem("idempotency-key-reused", 422, "Mismatch") }
        val controller = TrustController(keys, factory, "desktop", { auditEnrollmentNow }, false)
        try {
            setContent { MaterialTheme { CompositionLocalProvider(LocalDensity provides Density(1f, 2f)) { TrustGate(controller) {} } } }
            waitUntil { controller.state.value == TrustState.Enrollment && !controller.busy.value }
            runBlocking { controller.runAction { controller.enroll(auditEnrollmentUri, "fixture-password") }!!.join() }
            onNodeWithText("This enrollment was already processed. Enter the original password, or choose Start over.").assertExists()
            onNodeWithText("Start over", substring = false).assertExists()
            assertNotNull(keys.attempt)
        } finally { controller.close() }
    }
}
