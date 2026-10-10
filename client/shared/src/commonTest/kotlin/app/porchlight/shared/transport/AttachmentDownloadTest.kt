package app.porchlight.shared.transport

import app.porchlight.shared.backups.DownloadHead
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.JsonObject
import kotlin.test.*

class AttachmentDownloadTest {
    private val limit = 25 * 1024 * 1024
    private fun api(engine: MockEngine, connectFailure: (Throwable) -> Boolean = { false }) = KtorApiTransport(
        HttpClient(engine), listOf("https://lan", "https://other.ts.net"),
        HeaderPolicy(ClientInfo("0.1.0"), { "UTC" }, { "key" }), connectFailure)

    @Test fun attachmentAdapterUsesSharedStreamAndRestrictsItsPath() = runTest {
        var calls = 0
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = error("JSON not used")
            override suspend fun download(path: String, session: String?, range: String?, ifRange: String?, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
                calls++; assertEquals("/v1/attachments/a", path); assertEquals("token", session)
                assertNull(range); assertNull(ifRange)
                begin(DownloadHead(200, null, null, null))
                chunk(byteArrayOf(1, 2)); chunk(byteArrayOf(3))
            }
        }
        assertContentEquals(byteArrayOf(1, 2, 3), transport.download("/v1/attachments/a", "token"))
        assertFailsWith<IllegalArgumentException> { transport.download("/v1/backups/latest/download", "token") }
        assertEquals(1, calls)
    }

    @Test fun attachmentAdapterRejectsOversizeHeadersAndChunks() = runTest {
        for (length in listOf((limit + 1).toLong(), null, 1L)) {
            var chunks = 0
            val transport = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = error("JSON not used")
                override suspend fun download(path: String, session: String?, range: String?, ifRange: String?, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
                    begin(DownloadHead(200, null, length, null))
                    val bytes = ByteArray(64 * 1024)
                    repeat(limit / bytes.size) { chunks++; chunk(bytes) }
                    chunks++; chunk(byteArrayOf(1))
                    fail("Oversized stream accepted")
                }
            }
            assertEquals("attachment-too-large", assertFailsWith<ApiProblem> { transport.download("/v1/attachments/a", "token") }.type)
            assertEquals(if (length == (limit + 1).toLong()) 0 else 401, chunks)
        }
    }

    @Test fun attachmentDownloadUsesSameHeadersAndHandlesEmptyTinyAndExactCap() = runTest {
        for (size in listOf(0, 3, limit)) {
            val bytes = ByteArray(size) { (it % 251).toByte() }
            var calls = 0
            val transport = api(MockEngine { request ->
                calls++; assertEquals("/v1/attachments/a", request.url.encodedPath)
                assertEquals("Bearer token", request.headers[HttpHeaders.Authorization])
                assertEquals("0.1.0", request.headers["Porchlight-Client-Version"])
                assertEquals("UTC", request.headers["Porchlight-TZ"])
                assertNull(request.headers[HttpHeaders.Range]); assertNull(request.headers[HttpHeaders.IfRange])
                respond(bytes, HttpStatusCode.OK, headersOf(HttpHeaders.ContentLength, size.toString()))
            })
            try {
                assertContentEquals(bytes, transport.download("/v1/attachments/a", "token"))
                assertEquals(1, calls)
            } finally { transport.close() }
        }
    }

    @Test fun sharedStreamEnforcesAttachmentCapButNotBackupCap() = runTest {
        for (length in listOf((limit + 1).toString(), null, "1")) {
            var calls = 0; var delivered = 0
            val transport = api(MockEngine {
                calls++
                respond(ByteArray(limit + 1), HttpStatusCode.OK, if (length == null) headersOf() else headersOf(HttpHeaders.ContentLength, length))
            }, { true })
            try {
                val problem = assertFailsWith<ApiProblem> {
                    transport.download("/v1/attachments/a", "token", null, null, {}, { delivered += it.size })
                }
                assertEquals(413, problem.status); assertEquals(1, calls)
                // A chunk straddling the cap is rejected whole, so how much arrives depends on engine chunking.
                if (length == (limit + 1).toString()) assertEquals(0, delivered) else assertTrue(delivered in 1..limit)
                var total = 0
                transport.download("/v1/backups/latest/download", "token", null, null, {}, { total += it.size })
                assertEquals(limit + 1, total)
            } finally { transport.close() }
        }
    }

    @Test fun errorsAndCancellationAfterHeadersNeverReplay() = runTest {
        var calls = 0
        val denied = api(MockEngine {
            calls++; respond("""{"type":"urn:porchlight:problem:rate-limited","status":429,"title":"Wait"}""",
                HttpStatusCode.TooManyRequests, headersOf(HttpHeaders.ContentType to listOf("application/problem+json"), HttpHeaders.RetryAfter to listOf("3")))
        }, { true })
        try {
            val problem = assertFailsWith<ApiProblem> { denied.download("/v1/attachments/a", "token") }
            assertEquals(429, problem.status); assertEquals(3000L, problem.retryAfterMillis); assertEquals(1, calls)
        } finally { denied.close() }
        calls = 0
        val transport = api(MockEngine { calls++; respond(byteArrayOf(1), HttpStatusCode.OK) }, { true })
        val cancelled = CancellationException("cancelled")
        try {
            assertSame(cancelled, assertFailsWith<CancellationException> {
                transport.download("/v1/attachments/a", "token", null, null, {}, { throw cancelled })
            })
            assertEquals(1, calls)
            assertFailsWith<IllegalArgumentException> { transport.download("/v1/items", "token", null, null, {}, {}) }
            assertEquals(1, calls)
        } finally { transport.close() }
    }

    @Test fun connectFailureBeforeHeadersCanTryNextEndpoint() = runTest {
        var calls = 0
        val transport = api(MockEngine {
            calls++
            if (calls == 1) throw IllegalStateException("connect failure")
            respond(byteArrayOf(1, 2), HttpStatusCode.OK)
        }, { it is IllegalStateException })
        try {
            assertContentEquals(byteArrayOf(1, 2), transport.download("/v1/attachments/a", "token"))
            assertEquals(2, calls)
        } finally { transport.close() }
    }
}
