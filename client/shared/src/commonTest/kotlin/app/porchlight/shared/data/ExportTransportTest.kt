package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.*
import kotlin.test.*

class ExportTransportTest {
    @Test fun binaryDownloadUsesPinnedTransportPolicyWithoutJsonDecode() = runTest {
        val expected = byteArrayOf(31, -117, 0, 1, 2, -1)
        val api = KtorApiTransport(HttpClient(MockEngine { request ->
            assertEquals("/v1/admin/export/household", request.url.encodedPath)
            assertEquals("Bearer session", request.headers[HttpHeaders.Authorization])
            assertEquals("step-up", request.headers["Porchlight-Step-Up"])
            assertEquals("1.0.0", request.headers["Porchlight-Client-Version"])
            respond(expected, headers = headersOf(HttpHeaders.ContentType, "application/gzip"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            val received = mutableListOf<Byte>()
            // MockEngine does real asynchronous I/O; do not let runTest fast-forward the five-minute
            // production idle deadline before the mock channel gets CPU time.
            withContext(Dispatchers.Default) { api.download("/v1/admin/export/household", "session", "step-up") { received += it.toList() } }
            assertContentEquals(expected, received.toByteArray())
        } finally { api.close() }
    }
    @Test fun relockAtDownloadEndAbortsBeforePublishing() = runTest {
        var locked = false
        var aborted = false
        var published = false
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {}
            override suspend fun download(path: String, session: String?, stepUp: String?, consume: suspend (ByteArray) -> Unit) { consume(byteArrayOf(1)); locked = true }
        }
        val call = GeneratedCall({ api }, { "session" }, { if (locked) throw HouseholdLocked() })
        val sink = object : ExportSink {
            override suspend fun write(bytes: ByteArray) {}
            override suspend fun finish(checkGate: () -> Unit) { checkGate(); published = true }
            override suspend fun abort() { aborted = true }
        }
        assertFailsWith<HouseholdLocked> { ExportRepository(call).member(sink) }
        assertTrue(aborted)
        assertFalse(published)
    }
    @Test fun downloadProblemRetainsRetryAfterWithoutReplaying() = runTest {
        var calls = 0
        val api = KtorApiTransport(HttpClient(MockEngine {
            calls++
            respond("""{"type":"urn:porchlight:problem:rate-limited","status":429,"title":"Rate limited"}""", HttpStatusCode.TooManyRequests,
                headersOf(HttpHeaders.ContentType to listOf("application/problem+json"), HttpHeaders.RetryAfter to listOf("7")))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            val problem = assertFailsWith<ApiProblem> { withContext(Dispatchers.Default) { api.download("/v1/export/member", "session", null) {} } }
            assertEquals(7000L, problem.retryAfterMillis)
            assertEquals(1, calls)
        } finally { api.close() }
    }

}
