package app.porchlight.shared.data

import app.porchlight.shared.api.models.ListResponse
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class AuditClientDataFix1ListFenceTest {
    @Test fun olderContentBearingOrphanCollectionCannotUndoSameVersionRedaction() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val content = listFixture().copy(itemId = collabId("item"), capabilities = listOf("view", "delete"))
        val redacted = content.copy(name = null, redacted = true, propertyEntries = emptyList(), capabilities = listOf("delete"))
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        var requests = 0
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                val row = if (requests++ == 0) { started.complete(Unit); release.await(); content } else redacted
                return buildJsonObject { put("items", WireJson.encodeToJsonElement(listOf(row))) }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
        try {
            cache.putList(content)
            val old = async { data.lists.refresh(includeOrphaned = true) }
            started.await(); data.lists.refresh(includeOrphaned = true)
            release.complete(Unit)
            assertEquals(listOf(redacted), old.await(), "older authorized orphan reply must not escape redaction")
            assertEquals(listOf(redacted), cache.view.value.lists)
        } finally { release.complete(Unit); cache.close() }
    }
}
