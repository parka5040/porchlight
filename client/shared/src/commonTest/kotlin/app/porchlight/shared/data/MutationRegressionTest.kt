package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class MutationRegressionTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun inProgressAndExhaustedContentionKeepIntentUntilReplaySucceeds() = runTest {
        for (type in listOf("idempotency-in-progress", "conflict")) {
            val keys = mutableListOf<String?>()
            val bodies = mutableListOf<String>()
            val transport = KtorApiTransport(HttpClient(MockEngine { req ->
                keys += req.headers["Idempotency-Key"]; bodies += req.body.toByteArray().decodeToString()
                if (keys.size <= 3) respond("""{"type":"urn:porchlight:problem:$type","status":409,"title":"Busy"}""", HttpStatusCode.Conflict,
                    headersOf(HttpHeaders.ContentType to listOf("application/problem+json"), HttpHeaders.RetryAfter to listOf("1")))
                else respond("""{"id":"committed"}""", headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
            try {
                val writes = WriteCoordinator(wait = { kotlinx.coroutines.delay(it) })
                var generated = 0
                val screen = ScreenWrites { "intent-${++generated}" }
                val started = testScheduler.currentTime
                screen.submit { key -> writes.execute(key) { transport.request("POST", "/v1/items", buildJsonObject { put("title", "Frozen") }, idempotencyKey = it) }.feedback() }
                assertEquals(3, keys.size, type)
                assertEquals("intent-1", screen.state.value.retryKey, type)
                assertEquals(2000L, testScheduler.currentTime - started)
                screen.submit { error("Must not replace pending intent") }
                screen.retry()
                assertTrue(screen.state.value.saved, type)
                assertEquals(listOf<String?>("intent-1", "intent-1", "intent-1", "intent-1"), keys)
                assertEquals(1, bodies.distinct().size)
                assertEquals(1, generated)
            } finally { transport.close() }
        }
    }
    @Test fun visibilityRemoving204IsSuccessAndEvictsCachedItem() = runTest {
        val cache = memoryCache { now }
        val transport = KtorApiTransport(HttpClient(MockEngine { respond("", HttpStatusCode.NoContent) }), listOf(testOrigin()),
            HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one"))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val screen = ScreenWrites { "visibility" }
            screen.submit { data.items.edit("one", 1, ItemEdit().text("visibility", "private"), key = it).feedback() }
            assertTrue(screen.state.value.saved, screen.state.value.error)
            assertNull(screen.state.value.retryKey)
            assertTrue(cache.view.value.items.isEmpty())
        } finally { transport.close(); cache.close() }
    }
    @Test fun regeneratedOccurrence204IsSuccessAndReconcilesParent() = runTest {
        val cache = memoryCache { now }
        var refreshes = 0
        val transport = KtorApiTransport(HttpClient(MockEngine { respond("", HttpStatusCode.NoContent) }), listOf(testOrigin()),
            HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })
        try {
            val removedReminder = ScheduledReminderRead("alarm-removed", "household", "self", "removed", "one", "reminder", now, now)
            val siblingReminder = removedReminder.copy(id = "alarm-sibling", occurrenceId = "sibling")
            cache.install(emptySnapshot(1).copy(items = listOf(itemRead("one")), scheduledReminders = listOf(removedReminder, siblingReminder)))
            cache.putOccurrence(occurrenceResponse("removed", "one"))
            cache.putOccurrence(occurrenceResponse("sibling", "one"))
            cache.replaceBoard(listOf(card("removed", "A").copy(id = "one"), card("unrelated", "B")))
            cache.replaceAgenda(listOf(agenda("removed", "one"), agenda("sibling", "one"), agenda("unrelated")))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            data.occurrences.afterWrite = { assertEquals("one", it); refreshes++ }
            val screen = ScreenWrites { "reopen" }
            screen.submit { data.occurrences.reopen("removed", 1, it).feedback() }
            assertTrue(screen.state.value.saved, screen.state.value.error)
            assertEquals(1, refreshes)
            assertEquals(listOf("sibling"), cache.view.value.occurrences.map { it.id })
            assertEquals(listOf("unrelated"), cache.view.value.board.map { it.occurrence?.id })
            assertEquals(setOf("sibling", "unrelated"), cache.view.value.agenda.map { it.agenda?.occurrenceId }.toSet())
            assertEquals(listOf("alarm-sibling"), cache.view.value.reminders.map { it.id })
            assertEquals("one", cache.view.value.items.single().id)
        } finally { transport.close(); cache.close() }
    }
}
