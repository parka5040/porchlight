package app.porchlight.shared.transport

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class BackupDownloadTest {
    @Test fun streamedRangeUsesVersionSessionAndIfRange() = runTest {
        val bytes = ByteArray(150_000) { (it % 251).toByte() }; var calls = 0
        val engine = MockEngine { request ->
            calls++; assertEquals("bytes=7-", request.headers[HttpHeaders.Range]); assertEquals("\"digest\"", request.headers[HttpHeaders.IfRange])
            assertEquals("Bearer token", request.headers[HttpHeaders.Authorization]); assertEquals("0.1.0", request.headers["Porchlight-Client-Version"])
            respond(bytes, HttpStatusCode.PartialContent, headersOf(HttpHeaders.ETag to listOf("\"digest\""), HttpHeaders.ContentLength to listOf(bytes.size.toString())))
        }
        val api = KtorApiTransport(HttpClient(engine), listOf("https://lan"), HeaderPolicy(ClientInfo("0.1.0"), { "UTC" }, { "key" }), { false })
        var total = 0
        try {
            api.download("/v1/backups/latest/download", "token", "bytes=7-", "\"digest\"", { assertEquals(206, it.status) }, {
                assertTrue(it.size <= 65536); assertContentEquals(bytes.copyOfRange(total, total + it.size), it); total += it.size
            })
            assertEquals(bytes.size, total); assertEquals(1, calls)
        } finally { api.close() }
    }
    @Test fun rejectedResponseAndConsumerFailureAreNotReplayed() = runTest {
        var calls = 0
        val engine = MockEngine { calls++; respond("body", HttpStatusCode.OK) }
        val api = KtorApiTransport(HttpClient(engine), listOf("https://lan", "https://other.ts.net"), HeaderPolicy(ClientInfo("0.1.0"), { "UTC" }, { "key" }), { true })
        try { assertFailsWith<IllegalStateException> { api.download("/v1/backups/latest/download", "token", null, null, {}, { error("local failure") }) }; assertEquals(1, calls) }
        finally { api.close() }
        calls = 0
        val denied = KtorApiTransport(HttpClient(MockEngine { calls++; respond("""{"type":"urn:porchlight:problem:forbidden","status":403,"title":"Denied"}""", HttpStatusCode.Forbidden, headersOf("Content-Type", "application/problem+json")) }),
            listOf("https://lan", "https://other.ts.net"), HeaderPolicy(ClientInfo("0.1.0"), { null }, { "key" }), { true })
        try { assertEquals(403, assertFailsWith<ApiProblem> { denied.download("/v1/backups/latest/download", "token", null, null, {}, {}) }.status); assertEquals(1, calls) }
        finally { denied.close() }
    }
}
