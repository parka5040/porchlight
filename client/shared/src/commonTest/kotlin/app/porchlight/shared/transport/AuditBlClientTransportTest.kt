package app.porchlight.shared.transport

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.plugins.HttpTimeout
import io.ktor.http.*
import io.ktor.utils.io.ByteChannel
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditBlClientTransportTest {
    private fun api(engine: MockEngine) = KtorApiTransport(HttpClient(engine) { install(HttpTimeout) },
        listOf("https://fixture.invalid", "https://fallback.invalid"),
        HeaderPolicy(ClientInfo("1"), { "UTC" }, { "key" }), { true })

    @Test fun streamProblemsKeepBoundedRetryAfter() = runTest {
        for ((header, expected) in listOf("3" to 3000L, "999999" to 86_400_000L, "-1" to null, "bad" to null)) {
            var calls = 0
            val transport = api(MockEngine {
                calls++
                respond("""{"type":"urn:porchlight:problem:rate-limited","status":429,"title":"Wait"}""",
                    HttpStatusCode.TooManyRequests, headersOf(HttpHeaders.ContentType to listOf("application/problem+json"),
                        HttpHeaders.RetryAfter to listOf(header)))
            })
            try {
                val problem = assertFailsWith<ApiProblem> { transport.stream(0, "session") {} }
                assertEquals(expected, problem.retryAfterMillis, "Stream Retry-After $header")
                assertEquals(1, calls)
            } finally { transport.close() }
        }
    }

    @Test fun stalledExportIsRetryableFailureNotCallerCancellation() = runTest {
        for (status in listOf(HttpStatusCode.OK, HttpStatusCode.TooManyRequests)) {
            val body = ByteChannel()
            var calls = 0
            val engine = MockEngine(MockEngineConfig().apply {
                dispatcher = StandardTestDispatcher(testScheduler)
                addHandler { calls++; respond(body, status, headersOf(HttpHeaders.ContentType, "application/problem+json")) }
            })
            val transport = api(engine)
            val attempt = async { runCatching { transport.download("/v1/export/member", "session", null) {} } }
            try {
                runCurrent(); assertEquals(1, calls)
                advanceTimeBy(300_001); runCurrent()
                val problem = assertIs<ApiProblem>(attempt.await().exceptionOrNull())
                assertEquals("export-timeout", problem.type)
                assertEquals(504, problem.status)
                assertEquals(1, calls)
            } finally { attempt.cancelAndJoin(); body.cancel(null); transport.close() }
        }
    }

    @Test fun truncatedAttachmentCannotBePublishedAsSuccess() = runTest {
        for (length in listOf("1", "3")) {
            var calls = 0
            val transport = api(MockEngine { calls++; respond(byteArrayOf(1, 2), HttpStatusCode.OK, headersOf(HttpHeaders.ContentLength, length)) })
            try {
                val problem = assertFailsWith<ApiProblem> { transport.download("/v1/attachments/a", "session") }
                assertEquals("invalid-response", problem.type)
                assertEquals(1, calls)
            } finally { transport.close() }
        }
    }

    @Test fun callerCancellationStopsChannelReadWithoutReplay() = runTest {
        val body = ByteChannel()
        var calls = 0
        val started = CompletableDeferred<Unit>()
        val transport = api(MockEngine(MockEngineConfig().apply {
            dispatcher = StandardTestDispatcher(testScheduler)
            addHandler { calls++; respond(body, HttpStatusCode.OK) }
        }))
        val attempt = launch { transport.download("/v1/attachments/a", "session", null, null, { started.complete(Unit) }, {}) }
        try {
            started.await(); attempt.cancelAndJoin(); runCurrent()
            assertTrue(attempt.isCancelled)
            assertEquals(1, calls)
            assertTrue(body.isClosedForRead, "Cancelling the caller releases the response channel")
        } finally { attempt.cancelAndJoin(); body.cancel(null); transport.close() }
    }

    @Test fun binaryErrorsPreserveStatusAndBackupTitle() = runTest {
        for (status in listOf(HttpStatusCode.NotFound, HttpStatusCode.Gone)) {
            val transport = api(MockEngine { respond("unavailable", status, headersOf(HttpHeaders.ContentType, "text/plain")) })
            try {
                val attachment = assertFailsWith<ApiProblem> { transport.download("/v1/attachments/a", "session") }
                assertEquals(status.value, attachment.status)
                assertEquals("Request failed", attachment.title)
                val backup = assertFailsWith<ApiProblem> { transport.download("/v1/backups/latest/download", "session", null, null, {}, {}) }
                assertEquals(status.value, backup.status)
                assertEquals("Backup download failed", backup.title)
            } finally { transport.close() }
        }
    }
}
