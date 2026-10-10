package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*

class PlanningClientTest {
    @Test fun offlinePlanningSnapshotIsAtomicAndEvicted() = runTest {
        val now = kotlin.time.Instant.parse("2026-10-06T09:00:00Z")
        val cache = memoryCache { now }
        val availability = AvailabilityResponse("01999999-0000-7000-8000-000000000001", 1, listOf(Window(0, "09:00:00", "17:00:00")), emptyList())
        try {
            cache.install(emptySnapshot(1).copy(availability = listOf(availability)), now)
            assertEquals(listOf(availability), cache.view.value.availability)
            assertFailsWith<IllegalStateException> {
                cache.installStaged(emptySnapshot(2), now) { throw IllegalStateException("staging") }
            }
            assertEquals(listOf(availability), cache.view.value.availability)
            cache.clear()
            assertTrue(cache.view.value.availability.isEmpty())
        } finally { cache.close() }
    }
    @Test fun delayedSnapshotDoesNotOverwriteNewAvailabilityReceipt() = runTest {
        val now = kotlin.time.Instant.parse("2026-10-06T09:00:00Z")
        val cache = memoryCache { now }
        val row = AvailabilityResponse("01999999-0000-7000-8000-000000000001", 1, emptyList(), emptyList())
        try {
            cache.install(emptySnapshot(1).copy(availability = listOf(row)), now)
            val mark = cache.receiptMark()
            cache.putAvailability(row.copy(version = 2, windows = listOf(Window(0, "09:00:00", "17:00:00"))))
            cache.installStaged(emptySnapshot(2).copy(availability = listOf(row)), now, receivedAfter = mark)
            assertEquals(2, cache.view.value.availability.single().version)
        } finally { cache.close() }
    }
    @Test fun availabilityPutRetriesContentionWithoutChangingVersionBodyOrKey() = runTest {
        val member = "00000000-0000-0000-0000-000000000001"
        val requests = mutableListOf<Triple<String?, Long?, JsonObject?>>()
        val reply = AvailabilityResponse(member, 10, listOf(Window(0, "09:00:00", "17:00:00")), emptyList())
        val call = GeneratedCall({ object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                assertEquals("/v1/members/$member/availability", path)
                requests += Triple(idempotencyKey, version, body)
                if (requests.size == 1) throw ApiProblem("conflict", 409, "Contention", retryAfterMillis = 0)
                return WireJson.encodeToJsonElement(reply).jsonObject
            }
        } }, { "session" })
        val repo = AvailabilityRepository(call, WriteCoordinator(wait = {}))
        repo.afterWrite = { throw ConnectFailure() }
        val result = assertIs<WriteResult.Success<AvailabilityResponse>>(repo.put(member, 9, listOf(Window(0, "09:00", "17:00")), "retained"))
        assertTrue(result.refreshPending)
        assertEquals(requests[0], requests[1])
        assertEquals(9L, requests[0].second)
        assertEquals("09:00:00", requests[0].third!!["windows"]!!.jsonArray[0].jsonObject["start"]!!.jsonPrimitive.content)
        assertEquals(2, requests.size) // failed refresh never repeats a committed write
    }
    @Test fun feedSsrfProblemDetailIsNotDiscarded() = runTest {
        val problem = ApiProblem("validation-failed", 422, "Invalid calendar URL", "Calendar URL is not a public address")
        val call = GeneratedCall({ object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                assertEquals("/v1/feeds", path)
                throw problem
            }
        } }, { "session" })
        val thrown = assertFailsWith<ApiProblem> { FeedRepository(call, WriteCoordinator()).add(SourceRequest("School", "https://127.0.0.1/calendar.ics"), "feed-key") }
        assertSame(problem, thrown)
        assertEquals("Calendar URL is not a public address", thrown.detail)
    }
    @Test fun availabilityWholeSecondsAndWeekdays() {
        assertEquals("09:00:00", normalizeAvailabilityTime("09:00"))
        validateAvailability(listOf(Window(0, "22:00:00", "06:00:00"))) // overnight is legal
        assertFailsWith<IllegalArgumentException> { validateAvailability(listOf(Window(7, "09:00:00", "17:00:00"))) }
        assertFailsWith<IllegalArgumentException> { normalizeAvailabilityTime("25:00") }
        assertFailsWith<IllegalArgumentException> { normalizeAvailabilityTime("09:00:00.5") }
    }
    @Test fun previewAndCommitRetainBodyExceptPreview() = runTest {
        val requests = mutableListOf<Pair<JsonObject?, String?>>()
        val call = GeneratedCall({ object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                assertEquals("/v1/imports/ics", path)
                requests += body to idempotencyKey
                return WireJson.parseToJsonElement("""{"created":1,"updated":0,"unchanged":0,"removed":0,"deleted":0,"skipped":0,"warnings":[{"code":"UnsupportedProperty","message":"Unsupported property"}],"item_ids":[],"conflicts":[],"preview":${body!!["preview"]},"removal_suppressed":false}""").jsonObject
            }
        } }, { "session" })
        val repo = ImportRepository(call, WriteCoordinator())
        val request = ImportRequest("BEGIN:VCALENDAR\r\nEND:VCALENDAR", "00000000-0000-0000-0000-000000000001", name = "test.ics")
        assertTrue(repo.preview(request).preview)
        val result = repo.commit(request, "same-key") as WriteResult.Success
        assertFalse(result.value.preview)
        assertEquals("same-key", requests.last().second)
        assertEquals(requests[0].first!! - "preview", requests[1].first!! - "preview")
        assertTrue(importSummaryLines(result.value).last().endsWith("Unsupported property"))
    }
    @Test fun suggestionAcceptUsesOccurrenceVersionDismissUsesSuggestionVersion() = runTest {
        val versions = mutableListOf<Long?>()
        val keys = mutableListOf<String?>()
        val response = WireJson.parseToJsonElement("""{"id":"00000000-0000-0000-0000-000000000001","member_id":"00000000-0000-0000-0000-000000000002","item_id":"00000000-0000-0000-0000-000000000003","occurrence_id":"00000000-0000-0000-0000-000000000004","proposed_start":"2026-10-07T09:00:00Z","proposed_end":"2026-10-07T10:00:00Z","status":"proposed","run_id":"00000000-0000-0000-0000-000000000005","version":3,"occurrence_version":9,"created_at":"2026-10-06T09:00:00Z","conflicts":[]}""").jsonObject
        val call = GeneratedCall({ object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject { versions += version; keys += idempotencyKey; return response }
        } }, { "session" })
        val repo = PlanningRepository(call, WriteCoordinator())
        val row = WireJson.decodeFromJsonElement<SuggestionResponse>(response)
        repo.accept(row, "accept-key")
        repo.dismiss(row, "dismiss-key")
        assertEquals<List<Long?>>(listOf(9L, 3L), versions)
        assertEquals<List<String?>>(listOf("accept-key", "dismiss-key"), keys)
    }
}
