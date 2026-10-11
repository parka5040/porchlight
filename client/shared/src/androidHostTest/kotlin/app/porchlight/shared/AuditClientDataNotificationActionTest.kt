package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.live.retryNotificationAction
import app.porchlight.shared.transport.ApiProblem
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AuditClientDataNotificationActionTest {
    @Test fun exhaustedContentionRetriesOriginalFrozenOperation() = runTest {
        for (type in listOf("idempotency-in-progress", "conflict")) {
            val writes = WriteCoordinator(wait = {})
            val requests = mutableListOf<Pair<String, String>>()
            val frozenBody = "{\"status\":\"done\"}"
            suspend fun attempt() = writes.execute("original-key") { key ->
                requests += key to frozenBody
                if (requests.size <= 3) throw ApiProblem(type, 409, "Busy", retryAfterMillis = 1)
                Unit
            }
            val problem = assertFailsWith<ApiProblem> { attempt() }
            assertTrue(retryNotificationAction(problem), "worker must retry $type")
            assertIs<WriteResult.Success<Unit>>(attempt())
            assertEquals(List(4) { "original-key" to frozenBody }, requests)
        }
    }
    @Test fun ambiguousProxyRepliesRetryAndDefinitiveRejectionsStop() {
        for ((type, status) in listOf("http-error" to 403, "invalid-response" to 200, "unavailable" to 503))
            assertTrue(retryNotificationAction(ApiProblem(type, status, "Unknown")), type)
        for ((type, status) in listOf("version-conflict" to 409, "forbidden" to 403, "validation" to 422))
            assertFalse(retryNotificationAction(ApiProblem(type, status, "Rejected")), type)
    }
}
