package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class AuditBlClient3bCleanupTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private class Store : AttachmentStore by MemoryAttachmentStore() {
        val removed = mutableListOf<String>()
        val views = mutableListOf<String>()
        override suspend fun remove(key: String) { removed += key }
        override suspend fun removeView(id: String) { views += id }
    }
    @Test fun deletePurgesEachPrivateArtifactOnce() = runTest {
        val store = Store()
        val cache = memoryCache(store) { now }
        val transport = KtorApiTransport(HttpClient(MockEngine { respond("", HttpStatusCode.NoContent) }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { null }, { "unused" }), { false })
        try {
            val row = attachmentFixture()
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), attachments = listOf(row)))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
            data.attachments.afterWrite = {}
            assertIs<WriteResult.Success<Unit>>(data.attachments.delete(row))
            assertEquals(2, store.removed.size, "File and thumbnail should each be removed once")
            assertEquals(listOf(row.id), store.views)
        } finally { cache.close(); transport.close() }
    }
    @Test fun snapshotOmissionPurgesWithoutAnOptionalCollector() = runTest {
        val store = Store()
        val cache = memoryCache(store) { now }
        try {
            HouseholdData(cache, GeneratedCall({ error("No network") }, { null }), { now })
            val row = attachmentFixture()
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), attachments = listOf(row)))
            cache.install(emptySnapshot(2).copy(items = listOf(itemRead(row.itemId))))
            assertEquals(2, store.removed.size, "Snapshot omission must await private file cleanup")
            assertEquals(listOf(row.id), store.views)
        } finally { cache.close() }
    }
}
