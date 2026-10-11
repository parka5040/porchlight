package app.porchlight.shared.data

import app.porchlight.shared.screens.ScreenWrites
import app.porchlight.shared.screens.feedback
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.JsonObject
import kotlin.test.*

class AuditLimitsStorageTest {
    @Test fun uploadStorageFullExplainsHouseholdCapacityAndKeepsSafeRetry() = runTest {
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject = error("upload only")
            override suspend fun upload(path: String, name: String, mime: String, bytes: ByteArray, session: String?, key: String, progress: (Long, Long) -> Unit): JsonObject {
                throw ApiProblem("storage-full", 507, "Attachment storage is full")
            }
        }
        val cache = memoryCache { kotlin.time.Instant.parse("2026-10-06T12:00:00Z") }
        try {
            val repo = AttachmentRepository(GeneratedCall({ transport }, { null }), cache, WriteCoordinator())
            val screen = ScreenWrites { "upload-key" }
            screen.submit { repo.upload(collabId("item"), null, "notes.txt", "text/plain", byteArrayOf(65), it).feedback() }
            assertEquals("Attachment storage is full. Ask a household administrator to free space or increase the storage limit, then retry.", screen.state.value.error)
            assertEquals("upload-key", screen.state.value.retryKey)
            assertTrue(cache.view.value.attachments.isEmpty())
        } finally { cache.close() }
    }
}
