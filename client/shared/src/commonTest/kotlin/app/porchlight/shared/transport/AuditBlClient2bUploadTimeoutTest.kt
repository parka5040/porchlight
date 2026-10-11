package app.porchlight.shared.transport

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.plugins.*
import io.ktor.http.*
import kotlinx.coroutines.delay
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AuditBlClient2bUploadTimeoutTest {
    @Test fun responseProcessingCanOutlastUploadIdleWithoutReplaying() = runTest {
        var calls = 0
        val engine = MockEngine(MockEngineConfig().apply {
            dispatcher = kotlinx.coroutines.test.StandardTestDispatcher(testScheduler)
            addHandler { request ->
                calls++
                val timeouts = request.getCapabilityOrNull(HttpTimeoutCapability)
                assertEquals(600_000L, timeouts?.socketTimeoutMillis, "Response processing must not inherit the 30-second upload idle deadline")
                delay(31_000) // virtual ingest time after the request body was accepted
                respond("{}", headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }
        })
        val api = KtorApiTransport(HttpClient(engine) { install(HttpTimeout) }, listOf("https://lan"), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            api.upload("/v1/items/item/attachments", "fixture.txt", "text/plain", byteArrayOf(1), null, "fixture-operation") { _, _ -> }
            assertEquals(1, calls)
        } finally { api.close() }
    }
}
