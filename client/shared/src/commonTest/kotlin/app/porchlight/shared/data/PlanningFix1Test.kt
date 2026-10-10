package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import io.ktor.http.decodeURLQueryComponent
import kotlinx.serialization.json.*
import kotlin.test.*

class PlanningFix1Test {
    private val member = "01999999-0000-7000-8000-000000000001"
    private val response = WireJson.parseToJsonElement("""{"id":"01999999-0000-7000-8000-000000000002","member_id":"01999999-0000-7000-8000-000000000001","item_id":"01999999-0000-7000-8000-000000000003","occurrence_id":"01999999-0000-7000-8000-000000000004","proposed_start":"2026-10-07T09:00:00Z","proposed_end":"2026-10-07T10:00:00Z","status":"proposed","run_id":"01999999-0000-7000-8000-000000000005","version":3,"occurrence_version":9,"created_at":"2026-10-06T09:00:00Z","conflicts":[]}""").jsonObject
    @Test fun sealedStaleAcceptAndDismissRefetchAndRemoveActionableCard() = runTest {
        for ((field, code) in listOf("suggestion" to "not-proposed", "proposed_start" to "not-future", "member_id" to "not-agenda-member")) {
            val cache = memoryCache { kotlin.time.Instant.parse("2026-10-06T09:00:00Z") }
            val row = WireJson.decodeFromJsonElement<SuggestionResponse>(response)
            var reads = 0
            var writes = 0
            val problem = ApiProblem.decode("""{"type":"urn:porchlight:problem:validation-failed","status":422,"title":"Validation failed","errors":[{"field":"$field","code":"$code","message":"Proposal no longer applicable"}]}""")
            val call = GeneratedCall({ object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                    if (method == "GET") {
                        reads++; assertTrue(path.decodeURLQueryComponent().contains("member_id=$member"), path)
                        return buildJsonObject { put("items", JsonArray(emptyList())); put("next_cursor", JsonNull) }
                    }
                    writes++; throw problem
                }
            } }, { "session" })
            try {
                cache.install(emptySnapshot(1).copy(suggestions = listOf(row)))
                val repo = PlanningRepository(call, WriteCoordinator(), cache)
                assertFailsWith<ApiProblem> { repo.accept(row, "accept") }
                assertEquals(1, reads)
                assertTrue(cache.view.value.suggestions.isEmpty())
                assertFailsWith<ApiProblem> { repo.dismiss(row, "dismiss") }
                assertEquals(2, reads)
                assertEquals(2, writes) // no automatic resubmission with a new key
            } finally { cache.close() }
        }
    }
    @Test fun feedCreateAndEditSendTargetIntervalAndConditionalVersion() = runTest {
        val source = WireJson.parseToJsonElement("""{"id":"01999999-0000-7000-8000-000000000010","household_id":"01999999-0000-7000-8000-000000000011","owner_member_id":"$member","target_member_id":"$member","kind":"feed","visibility":"private","import_key":"feed:test","name":"School","url":"https://example.org/school.ics","allow_http":false,"refresh_seconds":3600,"enabled":true,"next_fetch_at":"2026-10-06T10:00:00Z","last_warnings":[],"failure_count":2,"version":4}""").jsonObject
        val methods = mutableListOf<String>()
        val call = GeneratedCall({ object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                methods += method
                assertEquals(member, body!!["target_member_id"]!!.jsonPrimitive.content)
                assertEquals(3600, body["refresh_seconds"]!!.jsonPrimitive.int)
                assertEquals("private", body["visibility"]!!.jsonPrimitive.content)
                if (method == "PUT") { assertEquals(4L, version); assertEquals("edit-key", idempotencyKey) }
                return source
            }
        } }, { "session" })
        val repo = FeedRepository(call, WriteCoordinator())
        val request = SourceRequest("School", "https://example.org/school.ics", member, SourceVisibility.private, refreshSeconds = 3600)
        val created = assertIs<WriteResult.Success<Source>>(repo.add(request, "create-key"))
        repo.update(created.value, request, "edit-key")
        assertEquals(listOf("POST", "PUT"), methods)
    }
    @Test fun itemAclEvictionRemovesOnlyItsPlanningRows() = runTest {
        val cache = memoryCache { kotlin.time.Instant.parse("2026-10-06T09:00:00Z") }
        val row = WireJson.decodeFromJsonElement<SuggestionResponse>(response)
        val unrelated = row.copy(id = "01999999-0000-7000-8000-000000000012", itemId = "01999999-0000-7000-8000-000000000013")
        try {
            cache.install(emptySnapshot(1).copy(suggestions = listOf(row, unrelated)))
            cache.evictItem(row.itemId)
            assertEquals(listOf(unrelated), cache.view.value.suggestions)
        } finally { cache.close() }
    }

}
