package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*

class ExportClientTest {
    @Test fun downloadUsesTrustTransportAndAbortsPartialSink() = runTest {
        var aborted = false
        var finished = false
        var downloaded = ""
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {}
            override suspend fun download(path: String, session: String?, stepUp: String?, consume: suspend (ByteArray) -> Unit) {
                assertEquals("/v1/export/member", path)
                assertEquals("live", session)
                consume("partial".encodeToByteArray())
                throw ConnectFailure()
            }
        }
        val repo = ExportRepository(GeneratedCall({ api }, { "live" }))
        val sink = object : ExportSink {
            override suspend fun write(bytes: ByteArray) { downloaded += bytes.decodeToString() }
            override suspend fun finish(checkGate: () -> Unit) { checkGate(); finished = true }
            override suspend fun abort() { aborted = true; downloaded = "" }
        }
        assertFailsWith<ConnectFailure> { repo.member(sink) }
        assertTrue(aborted)
        assertFalse(finished)
        assertEquals("", downloaded)
    }
    @Test fun revokeUsesStepUpAndStableKey() = runTest {
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                assertEquals("/v1/devices/01999999-0000-7000-8000-000000000009/revoke", path)
                assertEquals("device-step-up", stepUp)
                assertEquals("retained-key", idempotencyKey)
                return buildJsonObject { put("revoked_at", "2026-10-06T09:00:00Z") }
            }
        }
        assertIs<WriteResult.Success<*>>(DeviceAdminRepository(GeneratedCall({ api }, { "live" }), WriteCoordinator()).revoke("01999999-0000-7000-8000-000000000009", "device-step-up", "retained-key"))
    }
}
