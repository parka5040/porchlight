package app.porchlight.shared.transport

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class ReleaseDownloadTest {
    @Test fun nestedReleaseDownloadUsesVersionAndRangesButNoSession() = runTest {
        var calls = 0
        val api = KtorApiTransport(HttpClient(MockEngine { request ->
            calls++
            assertEquals("/v1/client-release/files/lib/operations.py", request.url.encodedPath)
            assertEquals("0.1.6", request.headers["Porchlight-Client-Version"])
            assertNull(request.headers[HttpHeaders.Authorization])
            assertEquals("bytes=2-", request.headers[HttpHeaders.Range])
            assertEquals("\"digest\"", request.headers[HttpHeaders.IfRange])
            respond("abc", HttpStatusCode.PartialContent, headersOf(HttpHeaders.ETag to listOf("\"digest\""),
                HttpHeaders.ContentLength to listOf("3"), HttpHeaders.ContentRange to listOf("bytes 2-4/5")))
        }), listOf("https://lan"), HeaderPolicy(ClientInfo("0.1.6"), { null }, { "unused" }), { false })
        var total = 0
        try {
            api.download("/v1/client-release/files/lib/operations.py", null, "bytes=2-", "\"digest\"", {
                assertEquals(206, it.status); assertEquals("bytes 2-4/5", it.contentRange)
            }, { total += it.size })
            assertEquals(3, total); assertEquals(1, calls)
        } finally { api.close() }
    }
}
