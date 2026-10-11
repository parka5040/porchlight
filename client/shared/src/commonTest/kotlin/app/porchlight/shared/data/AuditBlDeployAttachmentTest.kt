package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.MockEngine
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class AuditBlDeployAttachmentTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")

    @Test fun evictionCanFinishDuringPreparationAndPreventsLaunch() = runTest {
        val transport = KtorApiTransport(HttpClient(MockEngine { error("cached read only") }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        val store = object : AttachmentStore by MemoryAttachmentStore() {
            override suspend fun read(key: String) = byteArrayOf(1)
        }
        val cache = memoryCache(store) { now }
        var launched = false
        var discarded = false
        try {
            val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
            val row = attachmentFixture()
            cache.putAttachment(row)
            val failure = runCatching {
                data.attachments.openPrepared(row) {
                    // This would deadlock if preparation held the cache mutex. No timing assertion needed.
                    cache.evictAttachment(row.id)
                    object : PreparedAttachmentViewer {
                        override suspend fun launch() { launched = true }
                        override suspend fun discard() { discarded = true }
                    }
                }
            }.exceptionOrNull()
            assertNotNull(failure)
            assertFalse(launched)
            assertTrue(discarded)
            assertTrue(cache.view.value.attachments.isEmpty())
        } finally { transport.close(); cache.close() }
    }

    @Test fun failedLaunchDiscardsButSuccessfulLaunchRetainsViewerFile() = runTest {
        val transport = KtorApiTransport(HttpClient(MockEngine { error("cached read only") }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        val store = object : AttachmentStore by MemoryAttachmentStore() {
            override suspend fun read(key: String) = byteArrayOf(1)
        }
        val cache = memoryCache(store) { now }
        try {
            val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
            val row = attachmentFixture()
            cache.putAttachment(row)
            for (fail in listOf(false, true)) {
                var discarded = false
                val result = runCatching { data.attachments.openPrepared(row) {
                    object : PreparedAttachmentViewer {
                        override suspend fun launch() { if (fail) throw IllegalStateException("viewer unavailable") }
                        override suspend fun discard() { discarded = true }
                    }
                } }
                assertEquals(fail, result.isFailure)
                assertEquals(fail, discarded)
            }
        } finally { transport.close(); cache.close() }
    }
}
