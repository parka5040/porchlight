package app.porchlight.shared.screens

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.calendar.*
import app.porchlight.shared.screens.item.itemOccurrence
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class CalendarOccurrenceRegressionTest {
    @Test fun calendarTuesdayCompletionAndScopedEditsNeverTargetBoardMonday() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val monday = occurrenceRead("monday", "series").copy(version = 7)
        val tuesday = occurrenceRead("tuesday", "series").copy(version = 9)
        val parent = itemRead("series").copy(rrule = "FREQ=DAILY")
        val selectedRow = parent.copy(occurrence = tuesday)
        val view = CacheView(items = listOf(parent), board = listOf(parent.copy(occurrence = monday)), agenda = listOf(selectedRow))
        val route = calendarItemRoute(CalendarEntry(selectedRow, "09:00", "10:00"))
        val selected = assertNotNull(itemOccurrence(view, route.id, route.occurrenceId))
        val cache = memoryCache { now }
        val requests = mutableListOf<Triple<String, String?, String?>>()
        val transport = KtorApiTransport(HttpClient(MockEngine { req ->
            requests += Triple(req.url.encodedPath, req.url.parameters["occurrence_id"], req.headers["If-Match"])
            if (req.url.encodedPath == "/v1/items/series") respond(WireJson.encodeToString(itemResponse("series")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
            else respond(WireJson.encodeToString(occurrenceResponse("tuesday", "series")), headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            data.items.afterWrite = {}; data.occurrences.afterWrite = {}
            data.occurrences.complete(selected.id, selected.version, "complete")
            data.occurrences.patch(selected.id, selected.version, ItemEdit().text("override_local_due", "2026-10-06T11:00:00"), "edit-this")
            data.items.edit(parent.id, parent.version, ItemEdit().text("title", "Future"), EditScope.ThisAndFuture, selected.id, "edit-future")
            assertEquals(Triple("/v1/occurrences/tuesday/complete", null, "9"), requests[0])
            assertEquals(Triple("/v1/occurrences/tuesday", null, "9"), requests[1])
            assertEquals(Triple("/v1/items/series", "tuesday", "1"), requests[2])
            assertEquals("tuesday", route.occurrenceId)
            assertEquals(tuesday, itemOccurrence(view.copy(board = emptyList()), route.id, route.occurrenceId))
            assertNull(itemOccurrence(view.copy(agenda = emptyList()), route.id, "missing"))
        } finally { transport.close(); cache.close() }
    }
}
