package app.porchlight.shared.transport

import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.client.plugins.HttpTimeout
import io.ktor.client.plugins.HttpTimeoutCapability
import io.ktor.client.request.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AuditLimitsExportTimeoutTest {
    @Test fun exportsAllowFifteenMinutePreparationWithoutExtendingOtherDownloads() = runTest {
        val engine = MockEngine(MockEngineConfig().apply {
            dispatcher = kotlinx.coroutines.test.StandardTestDispatcher(testScheduler)
            addHandler { request ->
                val export = request.url.encodedPath.contains("/export/")
                assertEquals(if (export) 900_000L else 300_000L,
                    request.getCapabilityOrNull(HttpTimeoutCapability)?.socketTimeoutMillis,
                    "Export headers must allow the same preparation deadline as porch")
                respond("{}", HttpStatusCode.OK)
            }
        })
        val transport = KtorApiTransport(HttpClient(engine) {
            install(HttpTimeout) { connectTimeoutMillis = 10_000; socketTimeoutMillis = 300_000 }
        }, listOf("https://fixture.invalid"), HeaderPolicy(ClientInfo("1"), { "UTC" }, { "key" }), { false })
        try {
            transport.download("/v1/export/member", "session", null) {}
            transport.download("/v1/admin/export/household", "session", "step-up") {}
            transport.download("/v1/attachments/fixture", "session")
        } finally { transport.close() }
    }
}
