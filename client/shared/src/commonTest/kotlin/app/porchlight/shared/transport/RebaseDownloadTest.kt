package app.porchlight.shared.transport

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlin.test.*

class RebaseDownloadTest {
    @Test fun exportsDoNotInheritAttachmentCapAndRetainStepUpPolicy() = runTest {
        val bytes = ByteArray(25 * 1024 * 1024 + 1) { 7 }
        val api = KtorApiTransport(HttpClient(MockEngine { request ->
            assertEquals("Bearer session", request.headers[HttpHeaders.Authorization])
            assertEquals("UTC", request.headers["Porchlight-TZ"])
            assertEquals("1.0.0", request.headers["Porchlight-Client-Version"])
            assertNull(request.headers[HttpHeaders.Range])
            assertNull(request.headers[HttpHeaders.IfRange])
            assertEquals(if (request.url.encodedPath == "/v1/admin/export/household") "step-up" else null,
                request.headers["Porchlight-Step-Up"])
            respond(bytes, headers = headersOf(HttpHeaders.ContentLength, bytes.size.toString()))
        }), listOf("https://lan"), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            // Keep real MockEngine I/O off runTest's virtual idle-deadline clock.
            withContext(Dispatchers.Default) {
                for (path in listOf("/v1/export/member", "/v1/admin/export/household")) {
                    var total = 0
                    api.download(path, "session", if (path.endsWith("household")) "step-up" else null) {
                        assertTrue(it.all { byte -> byte == 7.toByte() })
                        total += it.size
                    }
                    assertEquals(bytes.size, total)
                }
                assertEquals("attachment-too-large", assertFailsWith<ApiProblem> {
                    api.download("/v1/attachments/a", "session")
                }.type)
            }
        } finally { api.close() }
    }

    @Test fun exportConsumerFailureNeverReplaysAndOverloadsKeepRouteRestrictions() = runTest {
        var calls = 0
        val failure = IllegalStateException("destination failed")
        val api = KtorApiTransport(HttpClient(MockEngine {
            calls++
            respond(byteArrayOf(1))
        }), listOf("https://lan", "https://other.ts.net"),
            HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { true })
        try {
            withContext(Dispatchers.Default) {
                assertSame(failure, assertFailsWith<IllegalStateException> {
                    api.download("/v1/export/member", "session", null) { throw failure }
                })
                assertEquals(1, calls)
                assertFailsWith<IllegalArgumentException> {
                    api.download("/v1/backups/latest/download", "session", null) {}
                }
                assertFailsWith<IllegalArgumentException> {
                    api.download("/v1/export/member", "session", null, null, {}, {})
                }
                assertEquals(1, calls)
            }
        } finally { api.close() }
    }
}
