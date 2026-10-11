package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class AuditClientDataFix2RequestFenceTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun requestTokensCannotBeReusedOutsideTheirOwnerAndContext() = runTest {
        val cache = memoryCache { now }; val other = memoryCache { now }
        try {
            val escaped = cache.inSession { it }
            assertFailsWith<IllegalStateException> { cache.install(escaped, emptySnapshot(1)) }
            cache.inSession { fence ->
                assertFailsWith<IllegalStateException> { other.install(fence, emptySnapshot(1)) }
                cache.install(fence, emptySnapshot(1))
                val retained = cache.retain(fence, "private result", emptyList())
                assertEquals("private result", retained.current(cache.view.value))
                cache.clear()
                assertNull(retained.current(cache.view.value))
            }
        } finally { cache.close(); other.close() }
    }
    @Test fun candidatesAreRejectedAtReceiptNotJustHiddenByTheScreen() = runTest {
        for (occurrence in listOf(false, true)) {
            val cache = memoryCache { now }
            val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
            val transport = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                    started.complete(Unit); release.await()
                    return WireJson.encodeToJsonElement(CandidatesResponse("item", emptyList(), false, "occurrence")).jsonObject
                }
            }
            val data = HouseholdData(cache, GeneratedCall({ transport }, { null }), { now })
            try {
                cache.inSession { cache.install(it, emptySnapshot(1).copy(items = listOf(itemRead("item")), occurrences = listOf(occurrenceRead("occurrence", "item")))) }
                val pending = async { runCatching { data.planning.candidates("item") } }
                started.await()
                if (occurrence) cache.evictOccurrence("occurrence") else cache.evictItem("item")
                release.complete(Unit)
                assertIs<StaleVisibilityReply>(pending.await().exceptionOrNull())
            } finally { release.complete(Unit); cache.close() }
        }
    }
}
