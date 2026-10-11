package app.porchlight.shared.transport

import app.porchlight.shared.data.ExportRepository
import app.porchlight.shared.data.ExportSink
import app.porchlight.shared.data.GeneratedCall
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.plugins.HttpTimeout
import io.ktor.http.*
import io.ktor.utils.io.ByteChannel
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditLimitsFix2ExportErrorTest {
    @Test fun stalledExportProblemBodyUsesOrdinaryDeadlineAndAbortsSink() = runTest {
        for (household in listOf(false, true)) {
            val body = ByteChannel()
            var requests = 0
            var aborted = false
            var finished = false
            val engine = MockEngine(MockEngineConfig().apply {
                dispatcher = StandardTestDispatcher(testScheduler)
                addHandler {
                    requests++
                    respond(body, HttpStatusCode.TooManyRequests,
                        headersOf(HttpHeaders.ContentType, "application/problem+json"))
                }
            })
            val transport = KtorApiTransport(HttpClient(engine) {
                install(HttpTimeout) { connectTimeoutMillis = 10_000; socketTimeoutMillis = 300_000 }
            }, listOf("https://fixture.invalid"), HeaderPolicy(ClientInfo("1"), { "UTC" }, { "key" }), { false })
            val repository = ExportRepository(GeneratedCall({ transport }, { "session" }, {}))
            val sink = object : ExportSink {
                override suspend fun write(bytes: ByteArray) { fail("Rejection must not write to the sink") }
                override suspend fun finish(checkGate: () -> Unit) { finished = true }
                override suspend fun abort() { aborted = true }
            }
            val download = async { runCatching {
                if (household) repository.household("step-up", sink) else repository.member(sink)
            } }
            try {
                runCurrent()
                assertEquals(1, requests, "Rejection headers arrive immediately")
                advanceTimeBy(299_999)
                runCurrent()
                assertFalse(download.isCompleted)
                advanceTimeBy(2)
                runCurrent()
                assertTrue(download.isCompleted, "Stalled export error body must stop at the ordinary 300s deadline")
                assertTrue(download.await().isFailure)
                assertTrue(aborted, "Timeout must abort the staged destination")
                assertFalse(finished)
                assertEquals(1, requests, "A body failure must not replay the request")
            } finally {
                download.cancelAndJoin()
                body.cancel(null)
                transport.close()
            }
        }
    }
}
