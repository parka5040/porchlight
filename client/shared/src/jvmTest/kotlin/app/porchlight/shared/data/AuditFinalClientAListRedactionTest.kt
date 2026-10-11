package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.*
import kotlin.time.Instant

class AuditFinalClientAListRedactionTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")

    @Test fun redactedCollectionWinsOverEqualVersionGetReceipt() = checkReceipt(entry = false, higher = false)
    @Test fun redactedCollectionWinsOverHigherVersionGetReceipt() = checkReceipt(entry = false, higher = true)
    @Test fun redactedCollectionNeverMergesEntryReceipt() = checkReceipt(entry = true, higher = false)
    @Test fun cleanupOnlyCollectionNeverMergesContentReceipts() = checkReceipt(entry = false, higher = true, cleanup = true)

    private fun checkReceipt(entry: Boolean, higher: Boolean, cleanup: Boolean = false) = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-redaction-")
        val file = root.resolve("cache.db").toFile()
        val cache = desktopReadCache(file) { now }
        val content = listFixture().copy(itemId = "item", propertyEntries = listOf(entryFixture("private-entry", "1")))
        val denied = content.copy(name = if (cleanup) "Cleanup descriptor" else null, redacted = !cleanup,
            propertyEntries = emptyList(), gotIt = emptyList(), capabilities = if (cleanup) listOf("view", "delete") else listOf("delete"))
        val getStarted = CompletableDeferred<Unit>(); val releaseGet = CompletableDeferred<Unit>()
        val refreshStarted = CompletableDeferred<Unit>(); val releaseRefresh = CompletableDeferred<Unit>()
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (path.substringBefore('?') == "/v1/lists") {
                    refreshStarted.complete(Unit); releaseRefresh.await()
                    return buildJsonObject { put("items", WireJson.encodeToJsonElement(listOf(denied))) }
                }
                getStarted.complete(Unit); releaseGet.await()
                return WireJson.encodeToJsonElement(content.copy(version = content.version + if (higher) 1 else 0)).jsonObject
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
        try {
            cache.putList(content)
            val earlier = async {
                if (entry) cache.inSession { fence ->
                    getStarted.complete(Unit); releaseGet.await()
                    cache.putEntry(fence, content.propertyEntries.single().copy(version = 2))
                } else data.lists.get(content.id)
            }
            getStarted.await()
            val refresh = async { data.lists.refresh(includeOrphaned = true) }
            refreshStarted.await(); releaseGet.complete(Unit); earlier.await()
            releaseRefresh.complete(Unit)
            assertEquals(listOf(denied), refresh.await(), "A collection visibility result must win over content receipts")
            assertEquals(listOf(denied), cache.view.value.lists, "Retained UI state must contain no private entries")
            cache.close()
            val reopened = desktopReadCache(file) { now }
            try { reopened.initialize(); assertEquals(listOf(denied), reopened.view.value.lists, "Reopened rows must remain private-free") }
            finally { reopened.close() }
        } finally { releaseGet.complete(Unit); releaseRefresh.complete(Unit); cache.close(); root.toFile().deleteRecursively() }
    }
}
