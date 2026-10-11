package app.porchlight.shared.data

import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class AttachmentVisibilityGranularityTest {
    @Test fun missingAttachmentConfirmsParentInsteadOfEvictingVisibleItem() = runTest {
        for (parentStatus in listOf(200, 404, 403, 503)) for (thumbnail in listOf(false, true)) {
            val now = Instant.parse("2026-10-06T12:00:00Z")
            var parentGets = 0
            val row = attachmentFixture().copy(hasThumbnail = true)
            val removed = mutableSetOf<String>()
            val store = object : AttachmentStore {
                override suspend fun read(key: String): ByteArray? = null
                override suspend fun write(key: String, bytes: ByteArray) = Unit
                override suspend fun clear() = Unit
                override suspend fun remove(key: String) { removed += key }
                override suspend fun removeView(id: String) { removed += id }
                override suspend fun cleanup(before: Instant) = Unit
            }
            val cache = memoryCache(store) { now }
            val api = KtorApiTransport(HttpClient(MockEngine { req ->
                val isParent = req.url.encodedPath == "/v1/items/${row.itemId}"
                if (isParent) parentGets++
                val status = if (isParent) parentStatus else 404
                if (status == 200) respond(WireJson.encodeToString(itemResponse(row.itemId)), headers = headersOf(HttpHeaders.ContentType, "application/json"))
                else respond("""{"type":"urn:porchlight:problem:${if (status == 403) "forbidden" else "not-found"}","status":$status,"title":"Unavailable"}""", HttpStatusCode.fromValue(status), headersOf(HttpHeaders.ContentType, "application/problem+json"))
            }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
            try {
                val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
                cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), comments = listOf(commentFixture()), attachments = listOf(row)))
                val problem = assertFailsWith<ApiProblem> { data.attachments.download(row, thumbnail) }
                assertEquals(404, problem.status)
                assertEquals(1, parentGets)
                assertTrue(cache.view.value.attachments.isEmpty())
                assertEquals(setOf("0-${row.id}-${row.version}-file", "0-${row.id}-${row.version}-thumb", row.id), removed)
                assertEquals(parentStatus == 200 || parentStatus == 503, cache.view.value.items.isNotEmpty(), "parent $parentStatus")
                assertEquals(parentStatus == 200 || parentStatus == 503, cache.view.value.comments.isNotEmpty(), "parent $parentStatus comments")
            } finally { api.close(); cache.close() }
        }
    }

    /** n-6: a session change during the download makes the attachment eviction cancel; that must propagate, not be
     * swallowed into a parent GET (and parent eviction) on behalf of the old session. */
    @Test fun sessionChangeDuringDeniedDownloadPropagatesCancellationWithoutParentGet() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }; var parentGets = 0; var epoch = 0L
        cache.sessionEpoch = { epoch }
        val row = attachmentFixture()
        val api = KtorApiTransport(HttpClient(MockEngine { req ->
            if (req.url.encodedPath == "/v1/items/${row.itemId}") parentGets++ else epoch = 1 // identity changed meanwhile
            respond("""{"type":"urn:porchlight:problem:not-found","status":404,"title":"Unavailable"}""", HttpStatusCode.NotFound, headersOf(HttpHeaders.ContentType, "application/problem+json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead(row.itemId)), comments = listOf(commentFixture()), attachments = listOf(row)))
            assertFailsWith<kotlinx.coroutines.CancellationException> { data.attachments.download(row) }
            assertEquals(0, parentGets)
            assertEquals(listOf(row.itemId), cache.view.value.items.map { it.id })
        } finally { api.close(); cache.close() }
    }
}
