package app.porchlight.shared

import app.porchlight.shared.data.WriteResult
import app.porchlight.shared.screens.ScreenWrites
import app.porchlight.shared.transport.ApiProblem
import app.porchlight.shared.trust.stateForProblem
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.JsonNull
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class AuditInstallerMaintenanceTest {
    @Test fun maintenanceIsTemporaryAndRetryKeepsTheRequest() = runTest {
        val problem = ApiProblem("maintenance", 503, "Server is updating", retryAfterMillis = 1000)
        val writer = ScreenWrites { "same-key" }
        val keys = mutableListOf<String>()
        writer.submit { key ->
            keys += key
            if (keys.size == 1) throw problem
            WriteResult.Success(JsonNull)
        }
        assertEquals("Server is updating — try again shortly", writer.state.value.error)
        assertEquals("same-key", writer.state.value.retryKey)
        assertNull(stateForProblem(problem)) // Not sign-out, revocation or permanent client upgrade.
        writer.retry()
        assertEquals(listOf("same-key", "same-key"), keys)
        assertNull(writer.state.value.error)
    }
}
