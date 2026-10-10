package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class RepositoryBehaviorTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun transport(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
    @Test fun createAndReminderWriteRecoverAuthoritativeIdentityBySnapshot() = runTest {
        val cache = memoryCache { now }
        var identity = "initial-reminder"
        val api = transport { req ->
            val body = when (req.url.encodedPath) {
                "/v1/items", "/v1/items/one/reminders" -> {
                    if (req.url.encodedPath.endsWith("reminders")) identity = "replacement-reminder"
                    WireJson.encodeToString(itemResponse("one", listOf(15)))
                }
                "/v1/snapshot" -> WireJson.encodeToString(emptySnapshot(2).copy(items = listOf(itemRead("one").copy(reminders = listOf(ReminderRead(identity, "household", "one", 15, now))))))
                else -> WireJson.encodeToString(ItemReadPage(emptyList(), null))
            }
            respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            val create = WireJson.decodeFromJsonElement<CreateRequest>(WireJson.encodeToJsonElement(itemResponse("one", listOf(15))))
            assertIs<WriteResult.Success<ItemResponse>>(data.items.create(create, "create"))
            assertEquals("initial-reminder", cache.view.value.items.single().reminders.single().id)
            // Same offset does not imply same identity; the server may replace rows.
            assertIs<WriteResult.Success<ItemResponse>>(data.items.setReminders("one", 1, listOf(15), "reminders"))
            assertEquals("replacement-reminder", cache.view.value.items.single().reminders.single().id)
        } finally { api.close(); cache.close() }
    }
    @Test fun cancelIncompletePaginationDoesNotPublishFirstPage() = runTest {
        val cache = memoryCache { now }
        val secondPage = CompletableDeferred<Unit>()
        val api = transport { req ->
            if (req.url.parameters["cursor"] != null) { secondPage.complete(Unit); awaitCancellation() }
            respond(WireJson.encodeToString(ItemPage(listOf(itemResponse("partial")), "next")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("old"))))
            val prior = cache.view.value
            val job = launch { data.items.list() }
            secondPage.await(); job.cancelAndJoin()
            assertEquals(prior, cache.view.value)
        } finally { api.close(); cache.close() }
    }
    @Test fun successfulWriteReplayNoOpStatusAndComposedMove() = runTest {
        val cache = memoryCache { now }
        val keys = mutableListOf<String?>()
        val payloads = mutableListOf<JsonObject>()
        val occurrence = occurrenceResponse("occ", "one")
        // Server idempotency: the first request per key commits (version+1); a replay returns the stored response.
        var version = 1
        val stored = mutableMapOf<String, String>()
        val api = transport { req ->
            val key = checkNotNull(req.headers["Idempotency-Key"])
            keys += key
            assertEquals("1", req.headers["If-Match"])
            payloads += WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
            val body = stored.getOrPut(key) {
                // A no-op status (already todo) is an ordinary 200 with the unchanged version.
                if (req.url.encodedPath.contains("occurrences")) WireJson.encodeToString(occurrence)
                else WireJson.encodeToString(itemResponse("one").copy(version = ++version))
            }
            respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            data.items.afterWrite = {}; data.occurrences.afterWrite = {}
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), occurrences = listOf(occurrenceRead("occ", "one"))))
            val first = data.items.edit("one", 1, ItemEdit().text("title", "Title"), key = "replay")
            val replay = data.items.edit("one", 1, ItemEdit().text("title", "Title"), key = "replay")
            assertIs<WriteResult.Success<ItemResponse>>(first); assertEquals(first, replay)
            assertEquals(2, first.value.version)
            assertEquals(2, cache.view.value.items.single().version) // the replay re-applies the same commit, not a second
            assertEquals(listOf<String?>("replay", "replay"), keys)
            val noOp = data.occurrences.status("occ", 1, OccurrenceStatus.todo, "no-op")
            assertIs<WriteResult.Success<OccurrenceResponse>>(noOp)
            assertEquals("todo", payloads.last()["status"]?.jsonPrimitive?.content)
            assertEquals(1, cache.view.value.occurrences.single().version)
            assertEquals("todo", cache.view.value.occurrences.single().status)
            cache.replaceBoard(listOf(card("above", "A"), card("occ", "B"), card("below", "C")))
            val moved = data.occurrences.move("occ", 1, BoardDrop("todo", 1), "composed")
            assertIs<WriteResult.Success<OccurrenceResponse>>(moved)
            val move = payloads.last()
            assertEquals("todo", move["status"]?.jsonPrimitive?.content)
            assertEquals("above", move["board_move"]?.jsonObject?.get("after_id")?.jsonPrimitive?.content)
            assertEquals("below", move["board_move"]?.jsonObject?.get("before_id")?.jsonPrimitive?.content)
        } finally { api.close(); cache.close() }
    }
    @Test fun conflictsSurfaceAndVersionConflictKeepsCurrentWithoutUpdatingCache() = runTest {
        val cache = memoryCache { now }
        var stale = false
        val conflict = WireJson.decodeFromString<ConflictResponse>("""{"member_id":"self","occurrence_id":"occ","other_occurrence_id":null,"other_item_id":null,"other_title":"Busy","starts_at":"2026-10-06T12:00:00Z","ends_at":"2026-10-06T13:00:00Z","kind":"hard"}""")
        val api = transport {
            if (stale) respond("""{"type":"urn:porchlight:problem:version-conflict","status":409,"title":"Changed","current":{"id":"one","version":9}}""", HttpStatusCode.Conflict, headersOf(HttpHeaders.ContentType, "application/problem+json"))
            else respond(WireJson.encodeToString(itemResponse("one").copy(conflicts = listOf(conflict))), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now }); data.items.afterWrite = {}
            val ok = data.items.take("one", 1)
            assertIs<WriteResult.Success<ItemResponse>>(ok); assertEquals(1, ok.value.conflicts.size)
            val prior = cache.view.value
            stale = true
            val failed = data.items.take("one", 1)
            assertIs<WriteResult.VersionConflict>(failed)
            assertEquals(9, failed.current?.jsonObject?.get("version")?.jsonPrimitive?.int)
            assertEquals(prior, cache.view.value)
        } finally { api.close(); cache.close() }
    }
    @Test fun staleNeighbourRefetchesVisibleBoardAndUsesFreshKey() = runTest {
        val cache = memoryCache { now }
        val keys = mutableListOf<String?>()
        var attempts = 0
        var refetches = 0
        val api = transport { req ->
            if (req.url.encodedPath == "/v1/board") {
                refetches++
                respond(WireJson.encodeToString(ItemReadPage(listOf(card("fresh", "A")), null)), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            } else {
                keys += req.headers["Idempotency-Key"]
                if (++attempts == 1) respond("""{"type":"urn:porchlight:problem:validation-failed","status":422,"title":"Stale","errors":[{"field":"after_id","code":"stale-neighbour"}]}""", HttpStatusCode.UnprocessableEntity, headersOf(HttpHeaders.ContentType, "application/problem+json"))
                else {
                    val body = WireJson.parseToJsonElement(req.body.toByteArray().decodeToString()).jsonObject
                    assertEquals("fresh", body["board_move"]?.jsonObject?.get("after_id")?.jsonPrimitive?.content)
                    respond(WireJson.encodeToString(occurrenceResponse("occ", "one")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
                }
            }
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now }, WriteCoordinator(newKey = { "new-key" }))
            data.occurrences.afterWrite = {}
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"))))
            cache.replaceBoard(listOf(card("stale", "A")))
            assertIs<WriteResult.Success<OccurrenceResponse>>(data.occurrences.move("occ", 1, BoardDrop("todo", 1), "old-key"))
            assertEquals(listOf<String?>("old-key", "new-key"), keys); assertEquals(1, refetches)
        } finally { api.close(); cache.close() }
    }
    @Test fun paginationSearchAndPendingMemberDerivedRefetch() = runTest {
        val cache = memoryCache { now }
        val cursors = mutableListOf<String?>()
        var pending = true
        val member = WireJson.decodeFromString<MemberRead>("""{"id":"self","household_id":"household","display_name":"Self","role":"member","color":"red","initials":"S","tzid":"UTC","tz_source":"manual","version":1,"pending_derivation":true}""")
        val api = transport { req ->
            val body = when (req.url.encodedPath) {
                "/v1/items" -> {
                    cursors += req.url.parameters["cursor"]
                    val second = req.url.parameters["cursor"] != null
                    WireJson.encodeToString(ItemPage(listOf(itemResponse(if (second) "two" else "one")), if (second) null else "page-two"))
                }
                "/v1/snapshot" -> WireJson.encodeToString(emptySnapshot(2).copy(members = listOf(member.copy(pendingDerivation = pending))))
                "/v1/members/self" -> WireJson.encodeToString(member)
                "/v1/members" -> WireJson.encodeToString(MemberReadPage(listOf(member.copy(pendingDerivation = pending)), null))
                "/v1/search" -> {
                    assertEquals("a & b", req.url.parameters["q"])
                    val item = JsonObject(WireJson.encodeToJsonElement(itemRead("found")).jsonObject + ("entity_type" to JsonPrimitive("item")))
                    """{"items":[$item,{"entity_type":"list","id":"groceries","name":"Groceries"}],"next_cursor":null}"""
                }
                else -> WireJson.encodeToString(ItemReadPage(emptyList(), null))
            }
            respond(body, headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            assertEquals(listOf("one", "two"), data.items.list(kind = "todo").map { it.id })
            assertEquals(listOf(null, "page-two"), cursors)
            assertIs<WriteResult.Success<MemberRead>>(data.household.patchMember("self", 1, MemberPatch(tzid = "UTC")))
            assertTrue(cache.view.value.members.single().pendingDerivation)
            pending = false
            data.sync.refetchByIds(memberIds = setOf("self")) // C4 derived event contract.
            assertFalse(cache.view.value.members.single().pendingDerivation)
            val prior = cache.view.value
            val results = data.search.search("a & b") // oneOf item | list hits dispatch on entity_type
            assertEquals("found", assertIs<SearchHit.Item>(results.hits[0]).item.id)
            assertEquals("list", assertIs<SearchHit.Other>(results.hits[1]).entityType)
            assertEquals(prior, cache.view.value)
        } finally { api.close(); cache.close() }
    }
    @Test fun cancellationAndSessionChangeNeverPublishLateRepositoryResponse() = runTest {
        val cache = memoryCache { now }
        var epoch = 1L; cache.sessionEpoch = { epoch }
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val api = transport {
            started.complete(Unit); release.await()
            respond(WireJson.encodeToString(itemResponse("late")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        try {
            val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
            val request = async { data.items.get("late") }
            started.await(); epoch++; cache.clear(); epoch++; cache.install(emptySnapshot(2))
            release.complete(Unit)
            assertFailsWith<CancellationException> { request.await() }
            assertTrue(cache.view.value.items.isEmpty()); assertEquals(2, cache.sync.value?.cursor)
        } finally { api.close(); cache.close() }
    }
}
fun occurrenceResponse(id: String, item: String): OccurrenceResponse {
    val fields = WireJson.encodeToJsonElement(occurrenceRead(id, item)).jsonObject
    return WireJson.decodeFromJsonElement(JsonObject(fields + mapOf("capabilities" to JsonArray(listOf(JsonPrimitive("change_status"))), "pending_derivation" to JsonPrimitive(false), "conflicts" to JsonArray(emptyList()))))
}
